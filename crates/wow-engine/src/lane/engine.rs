use super::{LaneMessage, LaneState};
use crate::action::finalize;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wow_control_proto::WorkerToProxy;
use wow_domain::time::Millis;
use wow_domain::*;
use wow_infra::logging::structured::{DiagnosticLogger, DiagnosticStream};
use wow_policy::questing::{
    objectives::{ObjectiveResolution, resolve as resolve_objective, resolve_with_exclusions},
    tasks::{QuestWorkId, QuestWorkKey, QuestWorkRuntime},
};
use wow_state::{ProtocolObservation, Snapshot, reduce};

#[derive(Clone, Debug)]
struct PendingMovement {
    runtime: crate::movement::MovementRuntime,
    destination: WorldPosition,
    alternate_destinations: Vec<Vec3>,
    destination_map_known: bool,
    acceptable_range: f32,
    resume: Option<GameplayCommand>,
    resume_pending: Option<PendingQuestAction>,
    resume_origin: PlanOrigin,
    work: QuestWorkRuntime,
    last_step: Option<Instant>,
    purpose: MovementPurpose,
    started_at: Instant,
    last_progress_at: Instant,
    last_progress_position: Option<Vec3>,
    last_progress_log: Option<Instant>,
    progress_source: Option<&'static str>,
    progress_revision: Option<StateRevision>,
    last_player_client_time: Option<u32>,
    last_position_update_at: Instant,
    route_failures: u8,
    transport: Option<(
        crate::movement::transport::TransportTraversal,
        crate::movement::transport::TransportProgress,
    )>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlayerLifeStatus {
    Alive,
    Dead,
    Ghost,
    Unknown,
}

impl PlayerLifeStatus {
    fn requires_recovery(self) -> bool {
        matches!(self, Self::Dead | Self::Ghost)
    }
}

fn quest_item_gameobject_open_command(target: EntityId) -> GameplayCommand {
    GameplayCommand::CastGameObject {
        spell: QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID,
        target,
        report_use: true,
    }
}

fn group_follow_stop_distance(intent: &MissionIntent) -> f32 {
    let role = match intent {
        MissionIntent::Party { role } | MissionIntent::Raid { role } => *role,
        _ => GroupRole::Auto,
    };
    wow_policy::group::follow::role_stop_distance(role)
}

fn repair_detour_should_continue(
    condition: wow_state::inventory::EquipmentCondition,
    group: wow_state::group::GroupLifecycle,
) -> bool {
    group != wow_state::group::GroupLifecycle::Active
        && wow_policy::maintenance::equipment_needs_repair(condition)
}

fn nearby_sell_vendor(snapshot: &Snapshot) -> Option<EntityId> {
    let position = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)?;
    let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
    let sell_entries: BTreeSet<u32> = catalog
        .world()
        .vendor_services
        .iter()
        .filter(|service| service.can_sell)
        .map(|service| service.entry_id)
        .collect();
    snapshot
        .state
        .entities
        .0
        .values()
        .filter(|entity| {
            entity.kind == wow_state::entities::EntityKind::Unit
                && entity.interactable
                && sell_entries.contains(&entity.entry)
        })
        .filter_map(|entity| {
            entity
                .position
                .map(|vendor_position| (entity, vendor_position))
        })
        .filter(|(_, vendor_position)| {
            vendor_position.map == position.map
                && vendor_position.point.distance(position.point) <= 5.0
        })
        .min_by(|(_, left), (_, right)| {
            left.point
                .distance(position.point)
                .total_cmp(&right.point.distance(position.point))
        })
        .map(|(entity, _)| entity.id)
}

fn nearby_class_trainer(snapshot: &Snapshot) -> Option<EntityId> {
    let position = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)?;
    snapshot
        .state
        .entities
        .0
        .values()
        .filter(|entity| {
            entity.kind == wow_state::entities::EntityKind::Unit
                && entity.interactable
                && entity
                    .npc_flags
                    .is_some_and(wow_policy::maintenance::is_class_trainer_flags)
        })
        .filter_map(|entity| entity.position.map(|point| (entity, point)))
        .filter(|(_, trainer_position)| {
            trainer_position.map == position.map
                && trainer_position.point.distance(position.point) <= 5.0
        })
        .min_by(|(_, left), (_, right)| {
            left.point
                .distance(position.point)
                .total_cmp(&right.point.distance(position.point))
        })
        .map(|(entity, _)| entity.id)
}

fn nearby_profession_trainer(snapshot: &Snapshot) -> Option<EntityId> {
    nearby_profession_trainers(snapshot).into_iter().next()
}

fn nearby_profession_trainers(snapshot: &Snapshot) -> Vec<EntityId> {
    let mut trainers = snapshot
        .state
        .entities
        .0
        .values()
        .filter(|entity| {
            let id = entity.id;
            wow_policy::gathering::professions::is_nearby_profession_trainer(snapshot, id)
        })
        .map(|entity| entity.id)
        .collect::<Vec<_>>();
    let position = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player);
    trainers.sort_by(|left, right| {
        let distance = |id: &EntityId| {
            position
                .and_then(|player| {
                    snapshot
                        .state
                        .entities
                        .0
                        .get(id)?
                        .position
                        .map(|target| (player, target))
                })
                .map(|(player, target)| player.point.distance(target.point))
                .unwrap_or(f32::INFINITY)
        };
        distance(left).total_cmp(&distance(right))
    });
    trainers
}

fn remembered_class_trainer_destination(
    snapshot: &Snapshot,
    remembered: &BTreeMap<u32, (WorldPosition, Instant)>,
    now: Instant,
) -> Option<WorldPosition> {
    let position = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)?;
    remembered
        .get(&position.map)
        .filter(|(_, observed_at)| {
            now.saturating_duration_since(*observed_at) <= CLASS_TRAINER_MEMORY_MAX_AGE
        })
        .map(|(trainer, _)| *trainer)
        .filter(|trainer| {
            trainer.point.distance(position.point) <= CLASS_TRAINER_DETOUR_RADIUS_YARDS
        })
}

struct RoutePlanJob {
    token: crate::movement::ReplanToken,
    stamped: crate::runtime::Stamped<WorldPosition>,
    cancellation: crate::runtime::CancellationToken,
    started_at: Instant,
    deadline_exceeded: bool,
    task: JoinHandle<Result<(Vec3, wow_navigation::PlannedRoute), wow_navigation::NavigationError>>,
}

#[derive(Clone, Copy, Debug)]
struct PendingFacing {
    mover: Option<EntityId>,
    orientation: f32,
    tolerance: f32,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct PendingTravelPreparation {
    kind: wow_policy::travel::TravelAbilityKind,
    spell: u32,
    destination: WorldPosition,
    work_id: QuestWorkId,
    deadline: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MovementPurpose {
    SearchArea,
    ApproachGroundedTarget,
    SurvivalApproach,
    RepairVendor,
    ClassTrainer,
    ProfessionTrainer,
    Banker,
}

const TURN_IN_SEARCH_RANGE: f32 = 5.0;
const TURN_IN_SEARCH_RETRY_DISTANCE: f32 = 50.0;
const QUEST_START_ARRIVAL_RANGE: f32 = 5.0;
const QUEST_START_EMPTY_LOG_RADIUS_YARDS: f32 = 200.0;
const QUEST_START_HUB_SWEEP_RADIUS_YARDS: f32 = 40.0;
const BANK_ACTION_TIMEOUT: Duration = Duration::from_secs(8);
const BANK_RETRY_DELAY: Duration = Duration::from_secs(30);
const QUEST_SEARCH_ARRIVAL_RANGE: f32 = 18.0;
const QUEST_ITEM_SOURCE_SEARCH_RADIUS: f32 = 5_000.0;
const QUEST_SEARCH_ROAM_RADIUS: f32 = 25.0;
const QUEST_TOOL_SEARCH_RANGE: f32 = 12.0;
const MAX_INTERACTION_RETRIES: usize = 512;
const MAX_CORPSE_RECLAIM_ATTEMPTS: u8 = 3;
const LOOT_ERROR_MASTER_INV_FULL: u8 = 12;
const CORPSE_HOSTILE_CLEARANCE_YARDS: f32 = 18.0;
const ENGINE_TICK_INTERVAL: Duration = Duration::from_millis(250);
const MOVEMENT_STEP_INTERVAL: Duration = Duration::from_millis(100);
const MOVEMENT_STALL_TIMEOUT: Duration = Duration::from_secs(5);
const MOVEMENT_OPERATION_TIMEOUT: Duration = Duration::from_secs(90);
const QUEST_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(10);
const RECOVERY_VENDOR_BUY_PENDING_TIMEOUT: Duration = Duration::from_secs(30);
const COMBAT_POTION_FALLBACK_LOCKOUT: Duration = Duration::from_secs(60 * 60);
const COMBAT_HEALTHSTONE_RETRY: Duration = Duration::from_secs(5);
const COMBAT_EMERGENCY_HEALTH_ITEM_RETRY: Duration = Duration::from_secs(30);
const BATTLEGROUND_STATUS_RETRY: Duration = Duration::from_secs(15);
const BATTLEGROUND_ACTIVE_STATUS_RETRY: Duration = Duration::from_secs(30);
const BATTLEGROUND_QUEUE_START_DELAY: Duration = Duration::from_secs(2);
const BATTLEGROUND_QUEUE_RETRY: Duration = Duration::from_secs(30);
const BATTLEGROUND_PORT_RETRY: Duration = Duration::from_secs(15);
const BATTLEGROUND_EXIT_RETRY: Duration = Duration::from_secs(5);
const MOVEMENT_STEP_EARLY_TOLERANCE: Duration = Duration::from_millis(10);
const BASE_RUN_SPEED_YARDS_PER_SECOND: f32 = 7.0;
const ROUTE_PLAN_DEADLINE: Duration = wow_navigation::ROUTE_PLANNING_DEADLINE;
const CLASS_TRAINER_DETOUR_RADIUS_YARDS: f32 = 30.0;
const CLASS_TRAINER_MEMORY_MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);
const CLASS_TRAINER_TRAVEL_RETRY: Duration = Duration::from_secs(5 * 60);
const BANKER_DETOUR_RADIUS_YARDS: f32 = 30.0;
const BANKER_MEMORY_MAX_AGE: Duration = Duration::from_secs(30 * 60);
const BANKER_TRAVEL_RETRY: Duration = Duration::from_secs(5 * 60);
const CLASS_TRAINER_MONEY_RESERVE_COPPER: u64 = 1_000;
const PROFESSION_TRAINER_DETOUR_RADIUS_YARDS: f32 = 30.0;
const PROFESSION_TRAINER_TRAVEL_RETRY: Duration = Duration::from_secs(5 * 60);
const MAILBOX_ACTION_TIMEOUT: Duration = Duration::from_secs(15);
const MAILBOX_RETRY_DELAY: Duration = Duration::from_secs(30);

fn recovery_vendor_buy_pending_done(
    baseline_count: u32,
    current_count: u32,
    deadline: Instant,
    now: Instant,
) -> bool {
    current_count != baseline_count || deadline <= now
}

fn combat_potion_lockout_ready(lockout_until: Option<Instant>, now: Instant) -> bool {
    lockout_until.is_none_or(|deadline| deadline <= now)
}

fn waiting_for_mail_observation(
    pending_generation: u64,
    current_generation: u64,
    deadline: Instant,
    now: Instant,
) -> bool {
    pending_generation == current_generation && deadline > now
}

fn combat_item_retry_ready(retry_until: Option<Instant>, now: Instant) -> bool {
    retry_until.is_none_or(|deadline| deadline <= now)
}

fn combat_item_use_authorized(snapshot: &Snapshot, target: EntityId, recovery: bool) -> bool {
    let Some(player) = snapshot.state.session.character_guid.map(EntityId) else {
        return false;
    };
    recovery
        || snapshot
            .state
            .entities
            .0
            .get(&player)
            .and_then(wow_state::entities::EntityState::in_combat)
            == Some(true)
        || snapshot
            .state
            .entities
            .0
            .get(&target)
            .and_then(wow_state::entities::EntityState::in_combat)
            == Some(true)
}

fn combat_item_command(
    selection: wow_policy::combat::consumables::CombatItemUse,
    player: EntityId,
) -> GameplayCommand {
    GameplayCommand::UseItemInstance {
        item: selection.item,
        item_guid: selection.instance.guid,
        backpack_slot: selection.instance.backpack_slot,
        spell: selection.spell,
        target: Some(player),
        cast_count: 0,
    }
}

#[derive(Clone, Debug)]
enum PendingQuestAction {
    Combat {
        target: EntityId,
        target_state: Option<wow_state::entities::EntityState>,
        started: Instant,
        cycle: Duration,
    },
    CorpseLoot {
        target: EntityId,
        baseline_generation: u64,
        started: Instant,
    },
    CorpseLootDelay {
        target: EntityId,
        arrived_at: Instant,
    },
    Loot {
        item: u32,
        target: EntityId,
        baseline_count: u32,
        baseline_generation: u64,
        started: Instant,
    },
    Interact {
        target: EntityId,
        started: Instant,
    },
    QuestObjectUse {
        quest: u32,
        target: EntityId,
        objective: Option<usize>,
        item: Option<u32>,
        baseline_progress: u32,
        baseline_count: u32,
        started: Instant,
    },
    ControlActivation {
        target: EntityId,
        started: Instant,
    },
    QuestCredit {
        quest: u32,
        objective: usize,
        baseline: u32,
        target: EntityId,
        started: Instant,
        label: &'static str,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DispatchOutcome {
    Sent,
    DeferredMovement,
    DeferredSpatial,
    DeferredDismount,
    Rejected,
    TransportClosed,
}

struct PendingDismount {
    action: Option<ProposedAction>,
    quest_action: Option<PendingQuestAction>,
    mission_revision: MissionRevision,
    player: Option<EntityId>,
    started_at: Instant,
    last_sent_at: Option<Instant>,
    attempts: u8,
}

#[derive(Clone, Copy)]
struct GroupPullDelay {
    target: EntityId,
    group_generation: u64,
    map: Option<u32>,
    started_at: Instant,
}

const TRAVEL_FORM_SPELL_IDS: [u32; 2] = [783, 2645];
const DISMOUNT_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const DISMOUNT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DISMOUNT_ATTEMPTS: u8 = 5;
const PLAYER_RECOVERY_MAX: Duration = Duration::from_secs(32);
const PLAYER_RECOVERY_STALL: Duration = Duration::from_secs(6);
const PLAYER_RECOVERY_START_GRACE: Duration = Duration::from_secs(2);
const PLAYER_RECOVERY_RETRY: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
struct ActivePlayerRecovery {
    selection: wow_policy::maintenance::PlayerRecoveryItem,
    started_at: Instant,
    deadline: Instant,
    last_progress_at: Instant,
    last_resource: u32,
    aura_seen: bool,
}

pub struct LaneEngine {
    pub state: LaneState,
    rx: mpsc::Receiver<LaneMessage>,
    proxy: mpsc::Sender<WorkerToProxy>,
    next_action: u64,
    next_task: u64,
    next_work: u64,
    last_quest_step: Option<Instant>,
    pending_accept: Option<(u32, EntityId, Instant)>,
    pending_giver_interaction: Option<(EntityId, Instant)>,
    giver_retry_after: BTreeMap<EntityId, (u8, Instant)>,
    pending_turn_in: Option<(u32, Instant, &'static str)>,
    reward_metadata_waiting: Option<(u32, Instant)>,
    last_turn_in_search_query: Option<(u32, Instant)>,
    turn_in_search_failures: BTreeMap<u32, (u32, Vec3)>,
    pending_movement: Option<PendingMovement>,
    next_movement: u64,
    route_job: Option<RoutePlanJob>,
    pending_facing: Option<PendingFacing>,
    pending_dismount: Option<PendingDismount>,
    assumed_facing: Option<(EntityId, f32, Instant)>,
    pending_quest_action: Option<PendingQuestAction>,
    interaction_retry_after: BTreeMap<(u32, EntityId), (u8, Instant)>,
    search_attempts: BTreeMap<(u32, usize, u32), BTreeSet<(u32, u32, u32)>>,
    quest_start_search_attempts: BTreeSet<(u32, u32, u32, u32)>,
    search_retry_after: BTreeMap<(u32, usize, u32), Instant>,
    combat_target_retry_after: BTreeMap<EntityId, Instant>,
    search_roam_cursor: BTreeMap<(u32, usize, u32), u8>,
    current_work: Option<QuestWorkRuntime>,
    last_wait_reason: Option<String>,
    last_dispatch: DispatchOutcome,
    movement_controller: Option<wow_navigation::MovementController>,
    transport_routes: Option<wow_navigation::transports::ValidatedTransportRoutes>,
    runtime_tuning: wow_infra::config::runtime_data::RuntimeTuning,
    pending_travel_preparation: Option<PendingTravelPreparation>,
    travel_preparation_skipped: Option<(QuestWorkId, WorldPosition)>,
    diagnostics: Option<DiagnosticLogger>,
    credited_quest_targets: BTreeSet<(u32, usize, EntityId)>,
    loot_retry_after: BTreeMap<EntityId, (u8, Option<Instant>)>,
    bag_full_loot_targets: BTreeSet<EntityId>,
    bag_relief_list_pending: bool,
    bag_relief_sale_pending: Option<(EntityId, u32, Instant)>,
    bag_relief_rejected_sales: BTreeSet<EntityId>,
    queried_item_templates: BTreeSet<u32>,
    looted_corpses: BTreeSet<EntityId>,
    corpses_ready_to_loot: BTreeSet<EntityId>,
    los_blocked: BTreeSet<EntityId>,
    los_attempts: BTreeMap<EntityId, u8>,
    server_range_recovery: BTreeMap<EntityId, (crate::action::spatial::ServerRangeCorrection, u8)>,
    behind_reposition_pending: BTreeSet<(u32, EntityId)>,
    behind_retry_after: BTreeMap<(u32, EntityId), Instant>,
    behind_retry_cast_allowed: BTreeSet<(u32, EntityId)>,
    maintenance_retry_after: BTreeMap<(u32, EntityId), Instant>,
    active_player_recovery: Option<ActivePlayerRecovery>,
    combat_potion_lockout_until: Option<Instant>,
    combat_healthstone_retry_until: Option<Instant>,
    combat_emergency_health_item_retry_until: Option<Instant>,
    recovery_vendor_buy_pending: Option<(u32, u32, Instant)>,
    repair_pending: Option<(wow_state::inventory::EquipmentCondition, Instant)>,
    repair_retry_after: Option<Instant>,
    remembered_class_trainers: BTreeMap<u32, (WorldPosition, Instant)>,
    remembered_bankers: BTreeMap<u32, (WorldPosition, Instant)>,
    class_training_character: Option<u64>,
    last_player_level: Option<u32>,
    class_training_due: bool,
    class_trainer_travel_retry_after: Option<Instant>,
    profession_trainer_travel_retry_after: Option<Instant>,
    mailbox_list_pending: Option<(EntityId, u64, Instant)>,
    mailbox_source: Option<EntityId>,
    mail_action_pending: Option<(u64, Instant)>,
    mail_retry_after: Option<Instant>,
    bank_open_pending: Option<(EntityId, Instant)>,
    bank_deposit_pending: Option<(EntityId, EntityId, u32, Instant)>,
    bank_retry_after: Option<Instant>,
    bank_travel_retry_after: Option<Instant>,
    last_maintenance_tick: Option<Instant>,
    last_maintenance_status: Option<String>,
    post_combat_loot: Option<(EntityId, Instant, Option<wow_state::entities::EntityState>)>,
    battleground_status_retry_after: Option<Instant>,
    battleground_status_requested_at: Option<Instant>,
    battleground_queue_retry_after: Option<Instant>,
    battleground_port_retry_after: Option<Instant>,
    battleground_exit_retry_after: Option<Instant>,
    group_loot_roll_observed_at: BTreeMap<EntityId, Instant>,
    group_loot_votes_sent: std::collections::BTreeSet<EntityId>,
    last_recovery_action: Option<Instant>,
    last_logged_player_life_status: Option<PlayerLifeStatus>,
    corpse_reclaim_attempts: u8,
    corpse_recovery_generation: u64,
    last_survival_action: Option<(EntityId, Instant)>,
    fishing_cast_at: Option<Instant>,
    fishing_retry_after: Option<Instant>,
    group_pull_delay: Option<GroupPullDelay>,
}

impl LaneEngine {
    pub fn new(
        state: LaneState,
        rx: mpsc::Receiver<LaneMessage>,
        proxy: mpsc::Sender<WorkerToProxy>,
    ) -> Self {
        let corpse_recovery_generation = state.authoritative.life.recovery_generation;
        Self {
            state,
            rx,
            proxy,
            next_action: 1,
            next_task: 1,
            next_work: 1,
            last_quest_step: None,
            pending_accept: None,
            pending_giver_interaction: None,
            giver_retry_after: BTreeMap::new(),
            pending_turn_in: None,
            reward_metadata_waiting: None,
            last_turn_in_search_query: None,
            turn_in_search_failures: BTreeMap::new(),
            pending_movement: None,
            next_movement: 1,
            route_job: None,
            pending_facing: None,
            pending_dismount: None,
            assumed_facing: None,
            pending_quest_action: None,
            interaction_retry_after: BTreeMap::new(),
            search_attempts: BTreeMap::new(),
            quest_start_search_attempts: BTreeSet::new(),
            search_retry_after: BTreeMap::new(),
            combat_target_retry_after: BTreeMap::new(),
            search_roam_cursor: BTreeMap::new(),
            current_work: None,
            last_wait_reason: None,
            last_dispatch: DispatchOutcome::Rejected,
            movement_controller: None,
            transport_routes: None,
            runtime_tuning: wow_infra::config::runtime_data::RuntimeTuning::default(),
            pending_travel_preparation: None,
            travel_preparation_skipped: None,
            diagnostics: None,
            credited_quest_targets: BTreeSet::new(),
            loot_retry_after: BTreeMap::new(),
            bag_full_loot_targets: BTreeSet::new(),
            bag_relief_list_pending: false,
            bag_relief_sale_pending: None,
            bag_relief_rejected_sales: BTreeSet::new(),
            queried_item_templates: BTreeSet::new(),
            looted_corpses: BTreeSet::new(),
            corpses_ready_to_loot: BTreeSet::new(),
            los_blocked: BTreeSet::new(),
            los_attempts: BTreeMap::new(),
            server_range_recovery: BTreeMap::new(),
            behind_reposition_pending: BTreeSet::new(),
            behind_retry_after: BTreeMap::new(),
            behind_retry_cast_allowed: BTreeSet::new(),
            maintenance_retry_after: BTreeMap::new(),
            active_player_recovery: None,
            combat_potion_lockout_until: None,
            combat_healthstone_retry_until: None,
            combat_emergency_health_item_retry_until: None,
            recovery_vendor_buy_pending: None,
            repair_pending: None,
            repair_retry_after: None,
            remembered_class_trainers: BTreeMap::new(),
            remembered_bankers: BTreeMap::new(),
            class_training_character: None,
            last_player_level: None,
            class_training_due: false,
            class_trainer_travel_retry_after: None,
            profession_trainer_travel_retry_after: None,
            mailbox_list_pending: None,
            mailbox_source: None,
            mail_action_pending: None,
            mail_retry_after: None,
            bank_open_pending: None,
            bank_deposit_pending: None,
            bank_retry_after: None,
            bank_travel_retry_after: None,
            last_maintenance_tick: None,
            last_maintenance_status: None,
            post_combat_loot: None,
            battleground_status_retry_after: None,
            battleground_status_requested_at: None,
            battleground_queue_retry_after: None,
            battleground_port_retry_after: None,
            battleground_exit_retry_after: None,
            group_loot_roll_observed_at: BTreeMap::new(),
            group_loot_votes_sent: std::collections::BTreeSet::new(),
            last_recovery_action: None,
            last_logged_player_life_status: None,
            corpse_reclaim_attempts: 0,
            corpse_recovery_generation,
            last_survival_action: None,
            fishing_cast_at: None,
            fishing_retry_after: None,
            group_pull_delay: None,
        }
    }

    pub fn with_movement_controller(
        mut self,
        controller: wow_navigation::MovementController,
    ) -> Self {
        self.movement_controller = Some(controller);
        self
    }

    pub fn with_transport_routes(
        mut self,
        routes: wow_navigation::transports::ValidatedTransportRoutes,
    ) -> Self {
        self.transport_routes = Some(routes);
        self
    }

    pub fn with_runtime_tuning(
        mut self,
        tuning: wow_infra::config::runtime_data::RuntimeTuning,
    ) -> Self {
        self.runtime_tuning = tuning;
        self
    }

    pub fn with_diagnostics(mut self, diagnostics: DiagnosticLogger) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    fn diagnostic(&self, stream: DiagnosticStream, record_type: &str, fields: serde_json::Value) {
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.record(stream, self.state.lane, record_type, fields);
        }
    }

    fn action_requires_on_foot(command: &GameplayCommand) -> bool {
        matches!(
            command,
            GameplayCommand::Attack(_)
                | GameplayCommand::Cast { .. }
                | GameplayCommand::CastOnItem { .. }
                | GameplayCommand::AcceptQuest { .. }
                | GameplayCommand::TurnInQuest { .. }
                | GameplayCommand::RequestQuestReward { .. }
                | GameplayCommand::ChooseQuestReward { .. }
                | GameplayCommand::Interact(_)
                | GameplayCommand::UseGameObject(_)
                | GameplayCommand::CastGameObject { .. }
                | GameplayCommand::Gather(_)
                | GameplayCommand::VendorList { .. }
                | GameplayCommand::VendorBuy { .. }
                | GameplayCommand::VendorSell { .. }
                | GameplayCommand::RepairEquipment { .. }
                | GameplayCommand::TrainerList { .. }
                | GameplayCommand::TrainerBuy { .. }
                | GameplayCommand::BankActivate { .. }
                | GameplayCommand::BankDeposit { .. }
                | GameplayCommand::AuctionBuy { .. }
                | GameplayCommand::TradeAccept { .. }
                | GameplayCommand::MailboxList { .. }
                | GameplayCommand::MailTake { .. }
        )
    }

    fn observed_travel_forms(&self) -> Vec<u32> {
        let Some(player) = self.authoritative_player_id() else {
            return Vec::new();
        };
        let auras = self.state.authoritative.auras.spells(player);
        TRAVEL_FORM_SPELL_IDS
            .into_iter()
            .filter(|spell| auras.contains(spell))
            .collect()
    }

    fn authoritative_player_id(&self) -> Option<EntityId> {
        self.state
            .authoritative
            .session
            .character_guid
            .map(EntityId)
    }

    fn observed_mounted(&self) -> Option<bool> {
        self.authoritative_player_id()
            .and_then(|player| self.state.authoritative.entities.0.get(&player))
            .and_then(wow_state::entities::EntityState::mounted)
    }

    fn player_is_on_foot(&self) -> bool {
        self.observed_mounted() == Some(false) && self.observed_travel_forms().is_empty()
    }

    fn travel_cleanup_required(&self) -> bool {
        self.observed_mounted() == Some(true) || !self.observed_travel_forms().is_empty()
    }

    fn begin_travel_cleanup(&mut self) {
        if self.pending_dismount.is_none() {
            self.pending_dismount = Some(PendingDismount {
                action: None,
                quest_action: None,
                mission_revision: self.state.mission_revision,
                player: self.authoritative_player_id(),
                started_at: Instant::now(),
                last_sent_at: None,
                attempts: 0,
            });
        }
    }

    async fn service_pending_dismount(&mut self) -> bool {
        let Some(pending) = self.pending_dismount.as_ref() else {
            return true;
        };
        if !self.state.runnable() || self.state.activation != ActivationStage::Act {
            self.pending_dismount = None;
            return true;
        }
        if pending.mission_revision != self.state.mission_revision
            || pending.player != self.authoritative_player_id()
            || pending
                .action
                .as_ref()
                .is_some_and(|action| action.stamp.mission != self.state.stamp().mission)
        {
            tracing::info!(lane=?self.state.lane, command=?pending.action.as_ref().map(|action| &action.command), "travel-form cleanup canceled because its mission or player changed");
            self.pending_dismount = None;
            return true;
        }
        if self.player_is_on_foot() {
            let Some(mut pending) = self.pending_dismount.take() else {
                return true;
            };
            if let Some(mut action) = pending.action.take() {
                action.stamp = self.state.stamp();
                tracing::info!(lane=?self.state.lane, command=?action.command, attempts=pending.attempts, "authoritative on-foot state confirmed; resuming blocked action");
                if let Some(quest_action) = pending.quest_action.take() {
                    return self
                        .dispatch_quest_semantic(action.command, quest_action)
                        .await;
                }
                let _ = self.submit(action).await;
            }
            return true;
        }
        let now = Instant::now();
        let expired = self.pending_dismount.as_ref().is_some_and(|pending| {
            now.saturating_duration_since(pending.started_at) >= DISMOUNT_TIMEOUT
                || pending.attempts >= MAX_DISMOUNT_ATTEMPTS
        });
        if expired {
            let pending = self
                .pending_dismount
                .take()
                .expect("pending cleanup was checked");
            tracing::warn!(lane=?self.state.lane, command=?pending.action.map(|action| action.command), attempts=pending.attempts, timeout_ms=DISMOUNT_TIMEOUT.as_millis(), "travel-form cleanup stopped without authoritative on-foot confirmation");
            self.waiting(
                "travel-form cleanup was not confirmed; blocked action was canceled".into(),
            );
            return true;
        }
        let due = self.pending_dismount.as_ref().is_some_and(|pending| {
            pending
                .last_sent_at
                .is_none_or(|last| now.saturating_duration_since(last) >= DISMOUNT_RETRY_INTERVAL)
        });
        if due {
            let mounted = self.observed_mounted();
            if mounted == Some(true) {
                let _ = self
                    .propose_command(GameplayCommand::CancelMount, false)
                    .await;
            }
            for spell in self.observed_travel_forms() {
                let _ = self
                    .propose_command(GameplayCommand::CancelAura { spell }, false)
                    .await;
            }
            if let Some(pending) = self.pending_dismount.as_mut() {
                pending.last_sent_at = Some(now);
                pending.attempts = pending.attempts.saturating_add(1);
            }
        }
        self.waiting("travel-form cleanup is waiting for authoritative on-foot state".into());
        true
    }

    pub async fn run(mut self) {
        if let Some(routes) = &self.transport_routes {
            tracing::info!(lane=?self.state.lane, valid_transport_legs=routes.len(), "authored transport routes loaded");
        }
        let mut mission_tick = tokio::time::interval(ENGINE_TICK_INTERVAL);
        mission_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut movement_tick = tokio::time::interval(MOVEMENT_STEP_INTERVAL);
        movement_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = mission_tick.tick() => {
                    if !self.tick_mission().await { break; }
                }
                _ = movement_tick.tick() => {
                    if self.pending_movement.is_some() && !self.tick_movement().await { break; }
                }
                msg = self.rx.recv() => {
                    let Some(msg) = msg else { break; };
                    if !self.handle(msg).await { break; }
                }
            }
        }
    }

    async fn handle(&mut self, msg: LaneMessage) -> bool {
        match msg {
            LaneMessage::Observation(o) => {
                match &o {
                    ProtocolObservation::InventoryInstances { items } => {
                        let before = &self.state.authoritative.inventory.items;
                        let mut after = std::collections::BTreeMap::<u32, u32>::new();
                        for instance in items {
                            *after.entry(instance.item).or_default() += u32::from(instance.count);
                        }
                        if before != &after {
                            let gained: std::collections::BTreeMap<_, _> = after
                                .iter()
                                .filter_map(|(item, count)| {
                                    let increase = count.saturating_sub(
                                        before.get(item).copied().unwrap_or_default(),
                                    );
                                    (increase > 0).then_some((*item, increase))
                                })
                                .collect();
                            let lost: std::collections::BTreeMap<_, _> = before
                                .iter()
                                .filter_map(|(item, count)| {
                                    let decrease = count.saturating_sub(
                                        after.get(item).copied().unwrap_or_default(),
                                    );
                                    (decrease > 0).then_some((*item, decrease))
                                })
                                .collect();
                            tracing::info!(
                                lane=?self.state.lane,
                                stacks=items.len(),
                                free_slots=self.state.authoritative.inventory.free_slots,
                                ?gained,
                                ?lost,
                                "authoritative inventory snapshot changed"
                            );
                        }
                    }
                    ProtocolObservation::InventoryFreeSlots { count } => {
                        tracing::info!(
                            lane=?self.state.lane,
                            previous=self.state.authoritative.inventory.free_slots,
                            current=count,
                            "authoritative inventory free-slot count changed"
                        );
                    }
                    ProtocolObservation::LootOpened { target, ownership } => {
                        tracing::info!(
                            lane=?self.state.lane,
                            ?target,
                            ?ownership,
                            loot_generation=self.state.authoritative.inventory.loot_generation,
                            bot_loot_generation=self.state.authoritative.inventory.bot_loot_generation,
                            "authoritative loot window opened"
                        );
                    }
                    ProtocolObservation::LootClosed { ownership } => {
                        tracing::info!(
                            lane=?self.state.lane,
                            ?ownership,
                            target=?self.state.authoritative.inventory.current_loot,
                            loot_generation=self.state.authoritative.inventory.loot_generation,
                            bot_loot_generation=self.state.authoritative.inventory.bot_loot_generation,
                            "authoritative loot window closed"
                        );
                    }
                    ProtocolObservation::QuestProgress {
                        quest,
                        objectives,
                        complete,
                    } => {
                        tracing::info!(lane=?self.state.lane, quest, ?objectives, complete, "authoritative quest progress updated");
                    }
                    ProtocolObservation::QuestCompleted { quest } => {
                        tracing::info!(lane=?self.state.lane, quest, "authoritative quest completion received");
                    }
                    ProtocolObservation::QuestRemoved { quest } => {
                        tracing::info!(lane=?self.state.lane, quest, "quest removed from authoritative journal");
                    }
                    ProtocolObservation::QuestTurnInDialog { quest, dialog } => {
                        tracing::info!(lane=?self.state.lane, quest, giver=?dialog.giver, stage=?dialog.stage, "authoritative quest turn-in dialog updated");
                    }
                    _ => {}
                }
                let item_templates_to_query: Vec<u32> = match &o {
                    ProtocolObservation::InventoryInstances { items } => {
                        items.iter().map(|instance| instance.item).collect()
                    }
                    ProtocolObservation::EquippedItems {
                        items: Some(items), ..
                    } => items.values().copied().collect(),
                    ProtocolObservation::GroupLootRollStarted(request) => {
                        wow_policy::questing::rewards::missing_score_metadata(
                            &Snapshot::from_state(&self.state.authoritative),
                            &[request.item_id],
                        )
                    }
                    ProtocolObservation::ItemTemplate { item, .. }
                        if self
                            .state
                            .authoritative
                            .group
                            .loot_rolls
                            .iter()
                            .any(|request| request.item_id == *item) =>
                    {
                        wow_policy::questing::rewards::missing_score_metadata(
                            &Snapshot::from_state(&self.state.authoritative),
                            &[*item],
                        )
                    }
                    _ => Vec::new(),
                };
                if !item_templates_to_query.is_empty() {
                    let missing: Vec<u32> = item_templates_to_query
                        .into_iter()
                        .filter(|item| {
                            *item != 0
                                && !self
                                    .state
                                    .authoritative
                                    .inventory
                                    .item_metadata
                                    .contains_key(item)
                                && self.queried_item_templates.insert(*item)
                        })
                        .collect();
                    for item in missing {
                        self.propose_command(GameplayCommand::QueryItem { item }, false)
                            .await;
                    }
                }
                if matches!(
                    &o,
                    ProtocolObservation::EnteredWorld { .. }
                        | ProtocolObservation::WorldChanged { .. }
                ) {
                    self.group_pull_delay = None;
                }
                if matches!(&o, ProtocolObservation::LeftWorld) {
                    self.reset_session_work();
                }
                match &o {
                    ProtocolObservation::LootRejected {
                        target,
                        loot_type,
                        error,
                    } => {
                        let full_bag = *error == Some(LOOT_ERROR_MASTER_INV_FULL);
                        if full_bag {
                            self.bag_full_loot_targets.insert(*target);
                            self.loot_retry_after.insert(*target, (u8::MAX, None));
                            self.bag_relief_list_pending = false;
                            self.bag_relief_sale_pending = None;
                            self.bag_relief_rejected_sales.clear();
                        }
                        if !full_bag {
                            let (attempts, _) = self
                                .loot_retry_after
                                .get(target)
                                .copied()
                                .unwrap_or_default();
                            let attempts = attempts.saturating_add(1);
                            let retry_after = match attempts {
                                1 => Some(Instant::now() + Duration::from_secs(5)),
                                2 => Some(Instant::now() + Duration::from_secs(20)),
                                _ => None,
                            };
                            self.loot_retry_after
                                .insert(*target, (attempts, retry_after));
                        }
                        if matches!(
                            self.pending_quest_action,
                            Some(PendingQuestAction::Loot { target: pending, .. }
                                | PendingQuestAction::CorpseLoot { target: pending, .. }
                                | PendingQuestAction::CorpseLootDelay { target: pending, .. })
                                if pending == *target
                        ) {
                            self.pending_quest_action = None;
                        }
                        if self
                            .post_combat_loot
                            .as_ref()
                            .is_some_and(|(pending, _, _)| pending == target)
                            && *error == Some(LOOT_ERROR_MASTER_INV_FULL)
                        {
                            self.post_combat_loot = None;
                        }
                        if *error == Some(LOOT_ERROR_MASTER_INV_FULL) {
                            tracing::warn!(lane=?self.state.lane, ?target, loot_type, ?error, "loot was rejected because the inventory is full");
                        } else {
                            let (attempts, retry_after) = self
                                .loot_retry_after
                                .get(target)
                                .copied()
                                .unwrap_or_default();
                            tracing::warn!(lane=?self.state.lane, ?target, loot_type, ?error, attempts, ?retry_after, "loot request rejected; target will retry after a delay");
                        }
                    }
                    ProtocolObservation::LootOpened {
                        target,
                        ownership: wow_state::LootOwnership::Bot,
                    } => {
                        self.loot_retry_after.remove(target);
                    }
                    ProtocolObservation::LootOpened {
                        target,
                        ownership: wow_state::LootOwnership::Player,
                    } => {
                        self.cancel_player_superseded_loot(*target);
                    }
                    ProtocolObservation::EntityUpsert { entity }
                        if entity.health.is_some_and(|(current, _)| current > 0) =>
                    {
                        self.loot_retry_after.remove(&entity.id);
                        self.looted_corpses.remove(&entity.id);
                    }
                    ProtocolObservation::EntityRemoved { entity } => {
                        self.loot_retry_after.remove(entity);
                        self.looted_corpses.remove(entity);
                    }
                    _ => {}
                }
                if let ProtocolObservation::InventoryFreeSlots { count } = &o
                    && *count > 0
                {
                    for target in std::mem::take(&mut self.bag_full_loot_targets) {
                        self.loot_retry_after.remove(&target);
                    }
                    self.bag_relief_list_pending = false;
                    self.bag_relief_sale_pending = None;
                    self.bag_relief_rejected_sales.clear();
                }
                let vendor_listed = match &o {
                    ProtocolObservation::VendorOpened { vendor }
                    | ProtocolObservation::VendorInventory { vendor, .. } => Some(*vendor),
                    _ => None,
                };
                if let Some(vendor) = vendor_listed {
                    self.maintenance_retry_after.remove(&(0, vendor));
                    self.bag_relief_list_pending = false;
                }
                if let ProtocolObservation::TrainerList { trainer, .. } = &o {
                    self.maintenance_retry_after.remove(&(0, *trainer));
                }
                if let ProtocolObservation::SpellKnown { spell } = &o
                    && let Some(trainer) = self.state.authoritative.trainer.trainer
                    && self
                        .state
                        .authoritative
                        .trainer
                        .offers
                        .iter()
                        .any(|offer| offer.spell == *spell)
                {
                    self.maintenance_retry_after.remove(&(*spell, trainer));
                    self.maintenance_retry_after.remove(&(u32::MAX, trainer));
                }
                if let ProtocolObservation::CastFailed {
                    spell,
                    reason,
                    target,
                } = &o
                {
                    if let Some(target) = target {
                        let retry = if wow_policy::maintenance::is_persistent_pet_summon(*spell) {
                            Instant::now()
                                + if self.state.authoritative.pet.control_known {
                                    Duration::from_secs(15)
                                } else {
                                    Duration::from_secs(60)
                                }
                        } else {
                            wow_policy::maintenance::retry_deadline(Instant::now())
                        };
                        self.maintenance_retry_after
                            .insert((*spell, *target), retry);
                        match *reason {
                            47 => {
                                self.los_blocked.insert(*target);
                                let attempts = self.los_attempts.entry(*target).or_default();
                                *attempts = attempts.saturating_add(1);
                                tracing::info!(lane=?self.state.lane, spell, ?target, attempts=*attempts, "authoritative line-of-sight failure armed shared reposition recovery");
                            }
                            97 => {
                                let attempts = self
                                    .server_range_recovery
                                    .get(target)
                                    .map(|(_, n)| n.saturating_add(1))
                                    .unwrap_or(1);
                                self.server_range_recovery.insert(
                                    *target,
                                    (
                                        crate::action::spatial::ServerRangeCorrection::MoveCloser,
                                        attempts,
                                    ),
                                );
                                tracing::info!(lane=?self.state.lane, spell, ?target, attempts, "authoritative out-of-range failure armed shared range recovery");
                            }
                            128 => {
                                let attempts = self
                                    .server_range_recovery
                                    .get(target)
                                    .map(|(_, n)| n.saturating_add(1))
                                    .unwrap_or(1);
                                self.server_range_recovery.insert(
                                    *target,
                                    (
                                        crate::action::spatial::ServerRangeCorrection::MoveFarther,
                                        attempts,
                                    ),
                                );
                                tracing::info!(lane=?self.state.lane, spell, ?target, attempts, "authoritative too-close failure armed shared range recovery");
                            }
                            57 => {
                                let key = (*spell, *target);
                                let now = Instant::now();
                                if self
                                    .behind_retry_after
                                    .get(&key)
                                    .is_none_or(|deadline| *deadline <= now)
                                {
                                    self.behind_retry_after
                                        .insert(key, now + Duration::from_secs(10));
                                    self.behind_reposition_pending.insert(key);
                                    tracing::info!(lane=?self.state.lane, spell, ?target, "server requires a rear-arc position; one reposition and retry are armed");
                                } else {
                                    tracing::info!(lane=?self.state.lane, spell, ?target, "rear-arc retry is cooling down after a failed reposition");
                                }
                            }
                            _ => {}
                        }
                    }
                    if matches!(*reason, 47 | 57 | 95 | 97 | 128) {
                        // The semantic action did not happen. Do not wait for quest credit
                        // or interaction evidence that can never arrive.
                        self.pending_quest_action = None;
                        self.last_quest_step = None;
                    }
                }
                if let ProtocolObservation::QuestGiverStatus { giver, status } = &o {
                    if self
                        .state
                        .authoritative
                        .quests
                        .giver_status
                        .get(giver)
                        .is_some_and(|previous| previous != status)
                    {
                        self.giver_retry_after.remove(giver);
                    }
                    if self
                        .pending_accept
                        .is_some_and(|(_, pending_giver, _)| pending_giver == *giver)
                        && !quest_status_available(*status)
                    {
                        tracing::info!(lane=?self.state.lane, ?giver, status, "server quest-giver status changed after accept attempt");
                        self.pending_accept = None;
                    }
                }
                if let ProtocolObservation::QuestGiverListReceived { giver, offer_count } = &o {
                    self.pending_giver_interaction = None;
                    self.giver_retry_after
                        .insert(*giver, (1, Instant::now() + giver_retry_delay(1)));
                    tracing::info!(lane=?self.state.lane, ?giver, offer_count, "authoritative quest-giver list response received");
                }
                if let ProtocolObservation::QuestProgress { quest, .. } = &o {
                    if self
                        .pending_accept
                        .is_some_and(|(pending_quest, _, _)| pending_quest == *quest)
                    {
                        tracing::info!(lane=?self.state.lane, quest, "quest acceptance confirmed by authoritative quest journal");
                        self.pending_accept = None;
                    }
                }
                if let ProtocolObservation::QuestTurnInDialog { quest, .. }
                | ProtocolObservation::QuestRemoved { quest }
                | ProtocolObservation::QuestCompleted { quest } = &o
                {
                    if self
                        .pending_turn_in
                        .is_some_and(|(pending_quest, _, _)| pending_quest == *quest)
                    {
                        tracing::info!(lane=?self.state.lane, quest, "quest turn-in step confirmed by authoritative server state");
                        self.pending_turn_in = None;
                    }
                }
                let facing_observation = match &o {
                    ProtocolObservation::PlayerPosition { position, .. }
                        if self.state.authoritative.control.mover.is_none() =>
                    {
                        Some((
                            self.state
                                .authoritative
                                .session
                                .character_guid
                                .map(EntityId),
                            position.orientation,
                        ))
                    }
                    ProtocolObservation::ControlledMover {
                        mover,
                        position: Some(position),
                        ..
                    } => Some((*mover, position.orientation)),
                    _ => None,
                };
                let movement_position = match &o {
                    ProtocolObservation::PlayerPosition {
                        position,
                        client_time,
                        ..
                    } if self.state.authoritative.control.mover.is_none() => {
                        Some(("player", *position, Some(*client_time), None))
                    }
                    ProtocolObservation::ControlledMover {
                        mover,
                        position: Some(position),
                        ..
                    } => Some(("controlled_mover", *position, None, *mover)),
                    _ => None,
                };
                let owned_kill = match &o {
                    ProtocolObservation::CreatureKilled { killer, victim }
                        if self.state.authoritative.session.character_guid == Some(killer.0)
                            || self.state.authoritative.pet.guid == Some(*killer) =>
                    {
                        Some((*killer, *victim))
                    }
                    _ => None,
                };
                let updated_entity = match &o {
                    ProtocolObservation::EntityUpsert { entity } => Some(entity.id),
                    _ => None,
                };
                let listed_trainer = match &o {
                    ProtocolObservation::TrainerList { trainer, .. } => Some(*trainer),
                    _ => None,
                };
                let learned_spell = match &o {
                    ProtocolObservation::SpellKnown { spell } => Some(*spell),
                    _ => None,
                };
                if matches!(o, ProtocolObservation::EnteredWorld { .. }) {
                    self.remembered_bankers.clear();
                    self.bank_travel_retry_after = None;
                }
                let previous_level = self
                    .state
                    .authoritative
                    .session
                    .character_guid
                    .and_then(|guid| self.state.authoritative.entities.0.get(&EntityId(guid)))
                    .and_then(|player| player.level);
                let group_roll_started = match &o {
                    ProtocolObservation::GroupLootRollStarted(request) => Some(request.item),
                    _ => None,
                };
                let delta = reduce(&mut self.state.authoritative, o);
                let active_roll_items: std::collections::BTreeSet<_> = self
                    .state
                    .authoritative
                    .group
                    .loot_rolls
                    .iter()
                    .map(|request| request.item)
                    .collect();
                self.group_loot_roll_observed_at
                    .retain(|item, _| active_roll_items.contains(item));
                self.group_loot_votes_sent
                    .retain(|item| active_roll_items.contains(item));
                if let Some(item) = group_roll_started {
                    self.group_loot_roll_observed_at
                        .insert(item, Instant::now());
                    self.group_loot_votes_sent.remove(&item);
                }
                if let Some(id) = updated_entity
                    && let Some(entity) = self.state.authoritative.entities.0.get(&id)
                    && entity.kind == wow_state::entities::EntityKind::Unit
                    && entity.interactable
                    && entity
                        .npc_flags
                        .is_some_and(wow_policy::maintenance::is_class_trainer_flags)
                    && let Some(position) = entity.position
                {
                    self.remembered_class_trainers
                        .insert(position.map, (position, Instant::now()));
                }
                if let Some(id) = updated_entity
                    && let Some(entity) = self.state.authoritative.entities.0.get(&id)
                    && entity.kind == wow_state::entities::EntityKind::Unit
                    && entity.interactable
                    && entity
                        .npc_flags
                        .is_some_and(wow_policy::economy::bank::is_banker_flags)
                    && let Some(position) = entity.position
                {
                    self.remembered_bankers
                        .insert(position.map, (position, Instant::now()));
                }
                let character = self.state.authoritative.session.character_guid;
                if character != self.class_training_character {
                    let character_changed = self.class_training_character.is_some();
                    self.class_training_character = character;
                    self.last_player_level = None;
                    self.class_training_due = false;
                    self.class_trainer_travel_retry_after = None;
                    if character_changed {
                        self.remembered_class_trainers.clear();
                        self.remembered_bankers.clear();
                        self.bank_travel_retry_after = None;
                    }
                }
                if let Some(level) = character
                    .map(EntityId)
                    .and_then(|player| self.state.authoritative.entities.0.get(&player))
                    .and_then(|player| player.level)
                {
                    let baseline = if updated_entity == character.map(EntityId) {
                        previous_level.or(self.last_player_level)
                    } else {
                        self.last_player_level
                    };
                    if baseline.is_none_or(|previous| level > previous) {
                        self.class_training_due = true;
                        self.class_trainer_travel_retry_after = None;
                    }
                    self.last_player_level = Some(level);
                }
                if let Some(trainer) = listed_trainer {
                    let snapshot = Snapshot::from_state(&self.state.authoritative);
                    if snapshot.state.trainer.trainer == Some(trainer)
                        && snapshot.state.trainer.trainer_type == Some(0)
                    {
                        if !wow_policy::maintenance::class_training_has_eligible_offer(&snapshot) {
                            self.class_training_due = false;
                            self.class_trainer_travel_retry_after = None;
                        } else if wow_policy::maintenance::class_training_has_affordable_offer(
                            &snapshot,
                        ) {
                            self.class_trainer_travel_retry_after = None;
                        } else {
                            self.class_trainer_travel_retry_after =
                                Some(Instant::now() + CLASS_TRAINER_TRAVEL_RETRY);
                        }
                    }
                }
                if learned_spell.is_some_and(|spell| {
                    self.state.authoritative.trainer.trainer_type == Some(0)
                        && self
                            .state
                            .authoritative
                            .trainer
                            .offers
                            .iter()
                            .any(|offer| offer.spell == spell)
                }) {
                    let snapshot = Snapshot::from_state(&self.state.authoritative);
                    if !wow_policy::maintenance::class_training_has_eligible_offer(&snapshot) {
                        self.class_training_due = false;
                        self.class_trainer_travel_retry_after = None;
                    }
                }
                if let Some((killer, target)) = owned_kill {
                    let mut corpse = self.state.authoritative.entities.0.get(&target).cloned();
                    if let Some(corpse) = &mut corpse
                        && relocate_moved_mob_corpse(&self.state.authoritative, corpse)
                    {
                        if let Some(entity) = self.state.authoritative.entities.0.get_mut(&target) {
                            entity.position = corpse.position;
                        }
                        tracing::info!(lane=?self.state.lane, ?target, ?killer, position=?corpse.position, "using the nearby killer position for a moved mob corpse");
                    }
                    self.post_combat_loot = Some((target, Instant::now(), corpse));
                    tracing::info!(lane=?self.state.lane, ?target, "server kill log credited the player or pet; corpse loot queued before the next target");
                }
                if let Some(target) = updated_entity
                    && let Some(entity) = self.state.authoritative.entities.0.get(&target)
                {
                    let corpse_position = refresh_cached_post_combat_corpse(
                        &mut self.post_combat_loot,
                        target,
                        entity,
                    );
                    if let Some(position) = corpse_position
                        && let Some(entity) = self.state.authoritative.entities.0.get_mut(&target)
                    {
                        entity.position = Some(position);
                    }
                }
                if let Some((source, position, client_time, mover)) = movement_position {
                    let source_is_active = match source {
                        "player" => self.state.authoritative.control.mover.is_none(),
                        _ => self.state.authoritative.control.mover == mover,
                    };
                    if source_is_active {
                        self.observe_movement_position(
                            source,
                            position,
                            client_time,
                            delta.revision,
                        );
                    }
                }
                if let (Some(pending), Some((mover, orientation))) =
                    (self.pending_facing, facing_observation)
                    && pending.mover == mover
                    && crate::action::spatial::angular_distance(orientation, pending.orientation)
                        <= pending.tolerance
                {
                    tracing::info!(lane=?self.state.lane, orientation, "authoritative facing update confirmed; targeted action may be retried");
                    self.pending_facing = None;
                }
                if facing_observation.is_some_and(|(mover, _)| {
                    self.assumed_facing
                        .is_some_and(|(assumed_mover, _, _)| Some(assumed_mover) == mover)
                }) {
                    self.assumed_facing = None;
                }
                if !delta.changed.is_empty() {
                    tracing::debug!(lane=?self.state.lane, revision=?delta.revision, changed=?delta.changed, "authoritative state updated");
                    self.diagnostic(
                        DiagnosticStream::Transition,
                        "authoritative_state_changed",
                        serde_json::json!({
                            "revision": format!("{:?}", delta.revision),
                            "domains": format!("{:?}", delta.changed),
                        }),
                    );
                }
            }
            LaneMessage::ReplaceMission(m) => {
                self.state.mission = m;
                self.state.mission_revision = self.state.mission_revision.next();
                self.group_pull_delay = None;
                self.reset_battleground_timers();
                self.last_quest_step = None;
                self.pending_accept = None;
                self.pending_turn_in = None;
                self.reward_metadata_waiting = None;
                self.cancel_route_job();
                self.pending_movement = None;
                self.pending_facing = None;
                self.assumed_facing = None;
                self.pending_quest_action = None;
                self.current_work = None;
                self.credited_quest_targets.clear();
                self.loot_retry_after.clear();
                self.looted_corpses.clear();
                self.corpses_ready_to_loot.clear();
                self.los_blocked.clear();
                self.los_attempts.clear();
                self.server_range_recovery.clear();
                self.behind_reposition_pending.clear();
                self.behind_retry_after.clear();
                self.behind_retry_cast_allowed.clear();
                self.last_wait_reason = None;
                tracing::info!(lane=?self.state.lane, mission=?self.state.mission.intent, revision=?self.state.mission_revision, "mission installed in lane engine");
                self.diagnostic(
                    DiagnosticStream::Transition,
                    "mission_installed",
                    serde_json::json!({
                        "mission_id": self.state.mission.id.0,
                        "mission_kind": match &self.state.mission.intent {
                            MissionIntent::Idle => "idle",
                            MissionIntent::Quest => "quest",
                            MissionIntent::Gather { .. } => "gather",
                            MissionIntent::Grind { .. } => "grind",
                            MissionIntent::Battleground { .. } => "battleground",
                            MissionIntent::Party { .. } => "party",
                            MissionIntent::Raid { .. } => "raid",
                            MissionIntent::Goal { .. } => "goal",
                        },
                        "revision": format!("{:?}", self.state.mission_revision),
                    }),
                );
            }
            LaneMessage::SetPause(p) => {
                if self.state.pause != p {
                    self.state.pause = p;
                    self.diagnostic(
                        DiagnosticStream::Transition,
                        "pause_reasons_changed",
                        serde_json::json!({"pause_reasons": format!("{p:?}")}),
                    );
                }
            }
            LaneMessage::UpdatePause { set, clear } => {
                let previous = self.state.pause;
                self.state.pause.insert(set);
                self.state.pause.remove(clear);
                if self.state.pause != previous {
                    self.diagnostic(
                        DiagnosticStream::Transition,
                        "pause_reasons_changed",
                        serde_json::json!({
                            "previous": format!("{previous:?}"),
                            "current": format!("{:?}", self.state.pause),
                        }),
                    );
                }
            }
            LaneMessage::SetActivation(stage) => {
                if self.state.activation != stage {
                    let previous = self.state.activation;
                    self.state.activation = stage;
                    self.state.permission_revision = self.state.permission_revision.next();
                    self.diagnostic(
                        DiagnosticStream::Transition,
                        "activation_stage_changed",
                        serde_json::json!({
                            "previous": format!("{previous:?}"),
                            "current": format!("{stage:?}"),
                            "permission_revision": format!("{:?}", self.state.permission_revision),
                        }),
                    );
                }
            }
            LaneMessage::Ownership {
                generation,
                movement,
                bot_allowed,
            } => {
                if self.state.ownership != generation || self.state.movement_epoch != movement {
                    self.cancel_route_job();
                }
                if self.state.movement_epoch != movement {
                    self.pending_movement = None;
                    self.pending_quest_action = None;
                }
                self.state.ownership = generation;
                self.state.movement_epoch = movement;
                if bot_allowed {
                    self.state.pause.remove(PauseReasons::PLAYER_CONTROL)
                } else {
                    self.state.pause.insert(PauseReasons::PLAYER_CONTROL)
                }
                self.diagnostic(
                    DiagnosticStream::Transition,
                    "ownership_changed",
                    serde_json::json!({
                        "generation": generation.get(),
                        "movement_epoch": movement.get(),
                        "bot_allowed": bot_allowed,
                    }),
                );
            }
            LaneMessage::MovementFence(epoch) => {
                if self.state.movement_epoch != epoch {
                    self.state.movement_epoch = epoch;
                    self.cancel_route_job();
                    self.pending_movement = None;
                    self.pending_quest_action = None;
                    self.diagnostic(
                        DiagnosticStream::Transition,
                        "movement_fence_changed",
                        serde_json::json!({"movement_epoch": epoch.get()}),
                    );
                }
            }
            LaneMessage::Propose(a) => {
                if !self.submit(a).await {
                    return false;
                }
            }
            LaneMessage::Shutdown => return false,
        }
        true
    }

    async fn tick_mission(&mut self) -> bool {
        if !self.state.runnable() {
            self.pending_dismount = None;
            self.waiting(format!("lane paused by {:?}", self.state.pause));
            return true;
        }
        if self.state.activation != ActivationStage::Act {
            self.pending_dismount = None;
            self.waiting(format!("activation stage is {:?}", self.state.activation));
            return true;
        }
        if !self.state.authoritative.session.in_world {
            self.pending_dismount = None;
            self.waiting("configured world session is not yet authoritative".to_owned());
            return true;
        }
        let life_status = self.player_life_status();
        if self.last_logged_player_life_status != Some(life_status) {
            match life_status {
                PlayerLifeStatus::Dead => {
                    tracing::warn!(lane=?self.state.lane, "player death detected from authoritative health");
                }
                PlayerLifeStatus::Ghost => {
                    tracing::warn!(lane=?self.state.lane, ghost_aura_spell=wow_state::life::GHOST_AURA_SPELL_ID, "player ghost aura observed; entering death recovery");
                }
                PlayerLifeStatus::Alive
                    if self
                        .last_logged_player_life_status
                        .is_some_and(PlayerLifeStatus::requires_recovery) =>
                {
                    tracing::info!(lane=?self.state.lane, "player resurrection observed; resuming mission work");
                }
                PlayerLifeStatus::Alive | PlayerLifeStatus::Unknown => {}
            }
            self.last_logged_player_life_status = Some(life_status);
        }
        if life_status.requires_recovery() {
            self.pending_dismount = None;
            self.active_player_recovery = None;
            return self.tick_death_recovery().await;
        }
        self.corpse_reclaim_attempts = 0;
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        if self.tick_active_player_recovery(&snapshot, Instant::now()) {
            return true;
        }
        if let Some(attacker) = wow_policy::combat::engagement::survival_attacker(&snapshot) {
            if self
                .pending_dismount
                .as_ref()
                .and_then(|pending| pending.action.as_ref())
                .is_some_and(|action| action.origin == PlanOrigin::Recovery)
            {
                return self.service_pending_dismount().await;
            }
            self.pending_dismount = None;
            if self
                .pending_movement
                .as_ref()
                .is_some_and(|movement| movement.purpose == MovementPurpose::SurvivalApproach)
            {
                return true;
            }
            if self.pending_movement.is_some() {
                tracing::info!(lane=?self.state.lane, ?attacker, "survival attacker preempted voluntary movement");
                self.stop_active_movement_for_handoff("survival combat preemption")
                    .await;
            }
            self.pending_quest_action = None;
            return self.tick_survival(attacker).await;
        }
        if self
            .pending_movement
            .as_ref()
            .is_some_and(|movement| movement.purpose == MovementPurpose::SurvivalApproach)
        {
            tracing::info!(lane=?self.state.lane, "survival approach ended because no authoritative attacker remains");
            self.stop_active_movement_for_handoff("survival approach ended")
                .await;
        }
        if self.pending_dismount.is_some() && !self.service_pending_dismount().await {
            return false;
        }
        if self.pending_dismount.is_some() {
            return true;
        }
        if self.tick_group_loot_rolls().await {
            return true;
        }
        if let MissionIntent::Battleground { battleground } = self.state.mission.intent.clone() {
            return self.tick_battleground(true, battleground.as_deref()).await;
        }
        if self.has_battleground_queue_state() {
            return self.tick_battleground(false, None).await;
        }
        if self
            .pending_movement
            .as_ref()
            .is_some_and(|movement| movement.purpose == MovementPurpose::RepairVendor)
            && !repair_detour_should_continue(
                self.state.authoritative.inventory.equipment_condition,
                self.state.authoritative.group.lifecycle,
            )
        {
            self.stop_active_movement_for_handoff("repair detour is no longer safe or needed")
                .await;
        }
        if self
            .pending_movement
            .as_ref()
            .is_some_and(|movement| movement.purpose == MovementPurpose::ClassTrainer)
            && (!self.class_trainer_travel_allowed(&snapshot)
                || nearby_class_trainer(&snapshot).is_some())
        {
            self.stop_active_movement_for_handoff(
                "class trainer detour is no longer safe or needed",
            )
            .await;
        }
        if self
            .pending_movement
            .as_ref()
            .is_some_and(|movement| movement.purpose == MovementPurpose::ProfessionTrainer)
            && (!self.profession_trainer_travel_allowed(&snapshot)
                || nearby_profession_trainer(&snapshot).is_some())
        {
            self.stop_active_movement_for_handoff(
                "profession trainer detour is no longer safe or needed",
            )
            .await;
        }
        if self
            .pending_movement
            .as_ref()
            .is_some_and(|movement| movement.purpose == MovementPurpose::Banker)
            && (!self.bank_travel_allowed(&snapshot)
                || !wow_policy::economy::bank::trusted_nearby_bankers(&snapshot).is_empty())
        {
            self.stop_active_movement_for_handoff("banker detour is no longer safe or needed")
                .await;
        }
        if self.pending_movement.is_some() {
            return true;
        }
        if !self.bag_full_loot_targets.is_empty()
            && let Some(result) = self.tick_bag_relief(&snapshot).await
        {
            return result;
        }
        if self.maintenance_eligible() {
            if let Some(result) = self.tick_equipment_repair(&snapshot).await {
                return result;
            }
            if let Some(result) = self.tick_maintenance().await {
                return result;
            }
            if let Some(result) = self.tick_mailbox(&snapshot).await {
                return result;
            }
            if let Some(result) = self.tick_class_trainer_travel(&snapshot) {
                return result;
            }
            if let Some(result) = self.tick_profession_trainer_travel(&snapshot) {
                return result;
            }
        }
        match self.state.mission.intent.clone() {
            MissionIntent::Idle => true,
            MissionIntent::Quest => self.tick_quest().await,
            MissionIntent::Gather { resource } => {
                if resource.eq_ignore_ascii_case("fishing") {
                    self.tick_fishing().await
                } else {
                    self.tick_gather(&resource).await
                }
            }
            MissionIntent::Grind { creature } => self.tick_grind(&creature).await,
            MissionIntent::Battleground { battleground } => {
                self.tick_battleground(true, battleground.as_deref()).await
            }
            MissionIntent::Goal { text } => self.tick_goal(&text).await,
            MissionIntent::Party { .. } | MissionIntent::Raid { .. } => {
                self.tick_group_encounter().await
            }
        }
    }

    async fn tick_bag_relief(&mut self, snapshot: &Snapshot) -> Option<bool> {
        let Some(position) = self
            .state
            .authoritative
            .control
            .active_position(self.state.authoritative.position.player)
        else {
            self.waiting(
                "inventory is full; waiting for the active mover position before vendor travel"
                    .into(),
            );
            return Some(true);
        };
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let Some((vendor_entry, destination)) = catalog.nearest_vendor(
            wow_infra::world_knowledge::VendorKind::Sell,
            position.map,
            position.point,
        ) else {
            self.waiting(format!(
                "inventory is full; no known sell vendor is available on map {}",
                position.map
            ));
            return Some(true);
        };

        if let Some(vendor) = self
            .state
            .authoritative
            .entities
            .0
            .values()
            .filter(|entity| {
                entity.entry == vendor_entry
                    && entity.kind == wow_state::entities::EntityKind::Unit
                    && entity.interactable
            })
            .filter_map(|entity| entity.position.map(|p| (entity, p)))
            .filter(|(_, p)| p.map == position.map && p.point.distance(position.point) <= 5.0)
            .min_by(|(_, a), (_, b)| {
                a.point
                    .distance(position.point)
                    .total_cmp(&b.point.distance(position.point))
            })
            .map(|(entity, _)| entity.id)
        {
            if snapshot.state.inventory.vendor != Some(vendor) {
                if !self.bag_relief_list_pending {
                    self.bag_relief_list_pending = true;
                    return Some(
                        self.propose_command(GameplayCommand::VendorList { vendor }, false)
                            .await,
                    );
                }
                return Some(true);
            }

            if let Some((guid, baseline, started)) = self.bag_relief_sale_pending {
                let count = snapshot
                    .state
                    .inventory
                    .instances
                    .get(&guid)
                    .map_or(0, |instance| instance.count);
                if count < baseline {
                    self.bag_relief_sale_pending = None;
                } else if started.elapsed() < Duration::from_secs(5) {
                    return Some(true);
                } else {
                    self.bag_relief_rejected_sales.insert(guid);
                    self.bag_relief_sale_pending = None;
                }
            }
            if let Some(candidate) = wow_policy::economy::vendor::safe_gray_item_sales(snapshot)
                .into_iter()
                .filter(|candidate| !self.bag_relief_rejected_sales.contains(&candidate.guid))
                .next()
            {
                let sent = self
                    .propose_command(
                        GameplayCommand::VendorSell {
                            vendor,
                            item: candidate.item,
                            item_guid: candidate.guid,
                            count: candidate.count,
                        },
                        false,
                    )
                    .await;
                if sent {
                    self.bag_relief_sale_pending =
                        Some((candidate.guid, candidate.count, Instant::now()));
                }
                return Some(sent);
            }
            self.waiting("inventory is full; vendor visit found no safe gray items to sell".into());
            return Some(true);
        }

        if position.point.distance(destination) <= 5.0 {
            self.waiting(format!(
                "inventory is full; waiting for vendor entry {vendor_entry} to become visible"
            ));
            return Some(true);
        }
        let work = self.set_work(QuestWorkKey::TravelToObjective {
            quest: 0,
            objective: 0,
            destination: WorldPosition {
                map: position.map,
                point: destination,
                orientation: 0.0,
            },
        });
        self.queue_world_movement(
            WorldPosition {
                map: position.map,
                point: destination,
                orientation: 0.0,
            },
            5.0,
            None,
            None,
            PlanOrigin::SystemPolicy,
            work,
            MovementPurpose::SearchArea,
        );
        Some(true)
    }

    async fn tick_gather(&mut self, resource: &str) -> bool {
        if !self
            .state
            .mission
            .permissions
            .contains(PermissionSet::GATHER)
        {
            self.waiting("gather mission is not authorized".into());
            return true;
        }
        if resource.trim().is_empty() {
            self.waiting(
                "gather mission is waiting for an authorized resource and capability".into(),
            );
            return true;
        }
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let matching_nodes: Vec<_> = catalog
            .world()
            .gather_nodes
            .iter()
            .filter(|node| {
                wow_policy::gathering::targets::normalized_name(&node.name)
                    == wow_policy::gathering::targets::normalized_name(resource)
            })
            .collect();
        let learned_skill = |node: &&wow_infra::world_knowledge::GatherNode| {
            let skill = match node.kind {
                wow_infra::world_knowledge::GatheringKind::Mining => 186,
                wow_infra::world_knowledge::GatheringKind::Herbalism => 182,
                wow_infra::world_knowledge::GatheringKind::Fishing => 356,
            };
            wow_policy::gathering::capability::has_required_skill(
                &snapshot,
                skill,
                node.required_skill.min(u16::MAX as u32) as u16,
            )
        };
        let target = self
            .state
            .authoritative
            .entities
            .0
            .values()
            .filter(|entity| {
                entity.interactable
                    && matches!(entity.kind, wow_state::entities::EntityKind::GameObject)
            })
            .filter(|entity| wow_policy::gathering::targets::matches_resource(entity, resource))
            .filter(|entity| {
                matching_nodes
                    .iter()
                    .find(|node| node.entry_id == entity.entry)
                    .is_none_or(|node| learned_skill(node))
            })
            .min_by_key(|entity| entity.id);
        if let Some(target) = target {
            return self
                .propose_command(GameplayCommand::Gather(target.id), false)
                .await;
        }
        if !matching_nodes.is_empty() && !self.state.authoritative.professions.known {
            self.waiting(format!(
                "gather mission for {resource:?} is waiting for authoritative profession state"
            ));
            return true;
        }
        if let Some(position) = self.active_mover_position()
            && let Some(node) = matching_nodes
                .iter()
                .filter(|node| learned_skill(node))
                .find(|node| node.spawns.iter().any(|spawn| spawn.map_id == position.map))
        {
            let key = (node.entry_id, usize::MAX, 0);
            let spawn_points: Vec<Vec3> = matching_nodes
                .iter()
                .filter(|candidate| learned_skill(candidate))
                .flat_map(|candidate| candidate.spawns.iter())
                .filter(|spawn| spawn.map_id == position.map)
                .map(|spawn| Vec3::new(spawn.x, spawn.y, spawn.z))
                .collect();
            let candidates = bounded_candidates(spawn_points, position.point);
            let reached = candidates
                .iter()
                .copied()
                .find(|point| quest_search_arrived(position.point, *point));
            if let Some(destination) = self.next_search_destination(key, &candidates, reached) {
                let work = self.set_work(QuestWorkKey::TravelToObjective {
                    quest: 0,
                    objective: 0,
                    destination: WorldPosition {
                        map: position.map,
                        point: destination,
                        orientation: 0.0,
                    },
                });
                tracing::info!(lane=?self.state.lane, resource, entry=node.entry_id, x=destination.x, y=destination.y, z=destination.z, "gather mission traveling to a trusted local resource search hint");
                self.queue_search_movement(destination, work).await;
                return true;
            }
            let roam = self.next_search_roam_destination(key, position.point);
            let work = self.set_work(QuestWorkKey::TravelToObjective {
                quest: 0,
                objective: 0,
                destination: WorldPosition {
                    map: position.map,
                    point: roam,
                    orientation: 0.0,
                },
            });
            tracing::info!(lane=?self.state.lane, resource, entry=node.entry_id, "gather resource hints exhausted locally; searching a nearby area");
            self.queue_search_movement(roam, work).await;
            return true;
        }
        self.waiting(format!("waiting for an observed {resource} resource"));
        true
    }

    async fn tick_fishing(&mut self) -> bool {
        if !self
            .state
            .mission
            .permissions
            .contains(PermissionSet::GATHER)
            || !self.state.authoritative.capabilities.can_fish
        {
            self.waiting(
                "fishing is waiting for authoritative fishing permission and capability".into(),
            );
            return true;
        }
        let now = Instant::now();
        let player = self
            .state
            .authoritative
            .session
            .character_guid
            .map(EntityId);
        if let Some(cast_at) = self.fishing_cast_at {
            let bobber = player.and_then(|player| {
                self.state.authoritative.entities.0.values().find(|entity| {
                    entity.kind == wow_state::entities::EntityKind::GameObject
                        && entity
                            .name
                            .as_deref()
                            .is_some_and(|name| name.to_ascii_lowercase().contains("bobber"))
                        && entity.target == Some(player)
                })
            });
            if let Some(bobber) = bobber {
                self.fishing_cast_at = None;
                self.fishing_retry_after = Some(now + Duration::from_secs(3));
                return self
                    .propose_command(GameplayCommand::Gather(bobber.id), false)
                    .await;
            }
            if now.duration_since(cast_at) >= Duration::from_secs(20) {
                self.fishing_cast_at = None;
                self.fishing_retry_after = Some(now + Duration::from_secs(5));
                self.waiting("fishing attempt timed out; retry is delayed".into());
            } else {
                self.waiting("waiting for an owned fishing bobber".into());
            }
            return true;
        }
        if self.fishing_retry_after.is_some_and(|retry| now < retry) {
            self.waiting("fishing attempt is in its retry delay".into());
            return true;
        }
        if player.is_none() || self.state.authoritative.position.player.is_none() {
            self.waiting(
                "fishing is waiting for authoritative player identity and position".into(),
            );
            return true;
        }
        self.fishing_cast_at = Some(now);
        self.propose_command(GameplayCommand::Fish, false).await
    }

    async fn tick_grind(&mut self, creature: &str) -> bool {
        if !self
            .state
            .mission
            .permissions
            .contains(PermissionSet::COMBAT)
        {
            self.waiting("grind mission is not authorized".into());
            return true;
        }
        if creature.trim().is_empty() || self.player_is_dead() {
            self.waiting("grind mission is waiting for a valid living-player target".into());
            return true;
        }
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        if let Some(target) =
            wow_policy::missions::grind::select_named_grind_target(&snapshot, creature)
        {
            return self
                .propose_command(GameplayCommand::Attack(target), false)
                .await;
        }
        self.waiting(format!(
            "waiting for a safe nearby observed {creature} target with current position and survival evidence"
        ));
        true
    }

    async fn tick_battleground(&mut self, active: bool, battleground: Option<&str>) -> bool {
        use wow_state::battleground::BattlegroundQueueStatus as Status;

        let now = Instant::now();
        let label = battleground.unwrap_or("random battleground");
        if let Some(requested) = battleground
            && !requested.eq_ignore_ascii_case("random")
        {
            self.waiting(format!(
                "battleground {requested:?} is not supported by the queue command; use random"
            ));
            return true;
        }

        let queues = &self.state.authoritative.battleground.queues;
        if queues
            .values()
            .any(|queue| matches!(queue.status, Status::Unknown(_)))
        {
            self.waiting("battleground queue has an unknown server status; waiting for a known status update".into());
            return true;
        }
        let queue = queues.values().next();
        if queues.len() > 1 {
            self.waiting("multiple battleground queue slots are active; waiting for a single authoritative queue".into());
            return true;
        }

        if !active {
            let Some(queue) = queue else {
                self.reset_battleground_timers();
                return false;
            };
            let Some(type_id) = queue.battleground_type_id else {
                self.waiting(
                    "battleground queue is active without a known battleground type; waiting"
                        .into(),
                );
                return true;
            };
            return match queue.status {
                Status::WaitQueue | Status::WaitJoin => {
                    if self
                        .battleground_port_retry_after
                        .is_some_and(|deadline| now < deadline)
                    {
                        return true;
                    }
                    self.battleground_port_retry_after = Some(now + BATTLEGROUND_EXIT_RETRY);
                    self.propose_command(
                        GameplayCommand::BattlegroundPort {
                            battleground_type_id: type_id,
                            enter: false,
                        },
                        false,
                    )
                    .await
                }
                Status::InProgress | Status::WaitLeave => {
                    if self
                        .battleground_exit_retry_after
                        .is_some_and(|deadline| now < deadline)
                    {
                        return true;
                    }
                    self.battleground_exit_retry_after = Some(now + BATTLEGROUND_EXIT_RETRY);
                    self.propose_command(
                        GameplayCommand::BattlegroundLeave {
                            battleground_type_id: type_id,
                        },
                        false,
                    )
                    .await
                }
                Status::None => false,
                Status::Unknown(_) => unreachable!(),
            };
        }

        if let Some(queue) = queue {
            match queue.status {
                Status::WaitQueue => {
                    self.battleground_status_requested_at = None;
                    if self
                        .battleground_status_retry_after
                        .is_none_or(|deadline| now >= deadline)
                    {
                        self.battleground_status_retry_after =
                            Some(now + BATTLEGROUND_STATUS_RETRY);
                        return self
                            .propose_command(GameplayCommand::BattlegroundStatus, false)
                            .await;
                    }
                    self.waiting(format!("queued for {label}; waiting for an invitation"));
                    return true;
                }
                Status::WaitJoin => {
                    let safe_to_port = self.player_life_status() == PlayerLifeStatus::Alive
                        && self.state.authoritative.transport.attached == Some(false)
                        && self
                            .state
                            .authoritative
                            .session
                            .character_guid
                            .map(EntityId)
                            .and_then(|player| self.state.authoritative.entities.0.get(&player))
                            .is_some_and(|player| player.in_combat() == Some(false));
                    if !safe_to_port {
                        self.waiting("battleground invitation is waiting for a living, out-of-combat player on foot".into());
                        return true;
                    }
                    let Some(type_id) = queue.battleground_type_id else {
                        self.waiting("battleground invitation has no known type; waiting".into());
                        return true;
                    };
                    if self
                        .battleground_port_retry_after
                        .is_some_and(|deadline| now < deadline)
                    {
                        return true;
                    }
                    self.battleground_port_retry_after = Some(now + BATTLEGROUND_PORT_RETRY);
                    return self
                        .propose_command(
                            GameplayCommand::BattlegroundPort {
                                battleground_type_id: type_id,
                                enter: true,
                            },
                            false,
                        )
                        .await;
                }
                Status::InProgress => {
                    if wow_policy::battleground::match_is_ending(queue) {
                        self.waiting(
                            "battleground match ended; waiting for the server leave transition"
                                .into(),
                        );
                        return true;
                    }
                    if self
                        .battleground_status_retry_after
                        .is_none_or(|deadline| now >= deadline)
                    {
                        self.battleground_status_retry_after =
                            Some(now + BATTLEGROUND_ACTIVE_STATUS_RETRY);
                        return self
                            .propose_command(GameplayCommand::BattlegroundStatus, false)
                            .await;
                    }
                    return self.tick_battleground_combat().await;
                }
                Status::WaitLeave => {
                    let Some(type_id) = queue.battleground_type_id else {
                        self.waiting(
                            "battleground is waiting to leave without a known type; waiting".into(),
                        );
                        return true;
                    };
                    if self
                        .battleground_exit_retry_after
                        .is_some_and(|deadline| now < deadline)
                    {
                        return true;
                    }
                    self.battleground_exit_retry_after = Some(now + BATTLEGROUND_EXIT_RETRY);
                    return self
                        .propose_command(
                            GameplayCommand::BattlegroundLeave {
                                battleground_type_id: type_id,
                            },
                            false,
                        )
                        .await;
                }
                Status::None => {}
                Status::Unknown(_) => unreachable!(),
            }
        }

        if self
            .state
            .authoritative
            .position
            .player
            .is_some_and(|position| {
                wow_policy::battleground::is_wotlk_battleground_map(position.map)
            })
        {
            return self.tick_battleground_combat().await;
        }

        if self.battleground_status_requested_at.is_none()
            && self
                .battleground_queue_retry_after
                .is_some_and(|deadline| now < deadline)
        {
            self.waiting(
                "battleground queue request is waiting for the server status update".into(),
            );
            return true;
        }
        if self.battleground_status_requested_at.is_none() {
            self.battleground_status_requested_at = Some(now);
            self.battleground_status_retry_after = Some(now + BATTLEGROUND_STATUS_RETRY);
            return self
                .propose_command(GameplayCommand::BattlegroundStatus, false)
                .await;
        }
        if self
            .battleground_status_requested_at
            .is_some_and(|requested| now.duration_since(requested) < BATTLEGROUND_QUEUE_START_DELAY)
        {
            self.waiting(
                "battleground mission is checking for an existing queue before joining".into(),
            );
            return true;
        }
        self.battleground_queue_retry_after = Some(now + BATTLEGROUND_QUEUE_RETRY);
        self.battleground_status_requested_at = None;
        self.propose_command(GameplayCommand::BattlegroundJoinRandom, false)
            .await
    }

    fn has_battleground_queue_state(&self) -> bool {
        !self.state.authoritative.battleground.queues.is_empty()
    }

    fn reset_battleground_timers(&mut self) {
        self.battleground_status_retry_after = None;
        self.battleground_status_requested_at = None;
        self.battleground_queue_retry_after = None;
        self.battleground_port_retry_after = None;
        self.battleground_exit_retry_after = None;
    }

    fn fresh_group_loot_rolls(&self, now: Instant) -> Vec<(EntityId, u32)> {
        self.state
            .authoritative
            .group
            .loot_rolls
            .iter()
            .filter_map(|request| {
                let observed_at = self.group_loot_roll_observed_at.get(&request.item)?;
                (request.countdown_ms > 0
                    && now.saturating_duration_since(*observed_at)
                        < Duration::from_millis(u64::from(request.countdown_ms)))
                .then_some((request.item, request.item_slot))
            })
            .collect()
    }

    async fn tick_group_loot_rolls(&mut self) -> bool {
        use wow_policy::economy::loot::{
            GroupLootMethod, GroupLootPolicy, LootEligibility, LootRollChoices, LootRollRequest,
            LootRollVote,
        };
        use wow_state::group::GroupLootMethod as ObservedMethod;

        if !matches!(
            self.state.mission.intent,
            MissionIntent::Party { .. } | MissionIntent::Raid { .. }
        ) || !self
            .state
            .mission
            .permissions
            .contains(PermissionSet::GROUP)
        {
            return false;
        }
        let Some(method) = self
            .state
            .authoritative
            .group
            .loot_method
            .map(|method| match method {
                ObservedMethod::FreeForAll => GroupLootMethod::FreeForAll,
                ObservedMethod::RoundRobin => GroupLootMethod::RoundRobin,
                ObservedMethod::MasterLoot => GroupLootMethod::MasterLoot,
                ObservedMethod::GroupLoot => GroupLootMethod::GroupLoot,
                ObservedMethod::NeedBeforeGreed => GroupLootMethod::NeedBeforeGreed,
                ObservedMethod::Unknown(_) => GroupLootMethod::Unknown,
            })
        else {
            return false;
        };
        let now = Instant::now();
        let requests = self.state.authoritative.group.loot_rolls.clone();
        for request in &requests {
            if self.group_loot_votes_sent.contains(&request.item) {
                continue;
            }
            let Some(observed_at) = self.group_loot_roll_observed_at.get(&request.item).copied()
            else {
                continue;
            };
            let elapsed = now.saturating_duration_since(observed_at).as_millis() as u64;
            if elapsed >= u64::from(request.countdown_ms) || request.countdown_ms == 0 {
                continue;
            }
            let policy_request = LootRollRequest {
                item: request.item,
                item_id: request.item_id,
                expires_at_ms: u64::from(request.countdown_ms),
                choices: LootRollChoices {
                    pass: request.allows(0),
                    need: request.allows(1),
                    greed: request.allows(2),
                    disenchant: request.allows(3),
                },
            };
            let quality = self
                .state
                .authoritative
                .inventory
                .item_metadata
                .get(&request.item_id)
                .map(|metadata| metadata.quality);
            let loot_tuning = &self.runtime_tuning.group.loot;
            let loot_policy = GroupLootPolicy {
                need_usable_upgrades: loot_tuning.need_usable_upgrades,
                greed_non_upgrades: loot_tuning.greed_non_upgrades,
                disenchant_non_upgrades: loot_tuning.disenchant_non_upgrades,
                max_need_quality: loot_tuning.max_need_quality(),
            };
            let usable_upgrade = wow_policy::gear::usable_equipment_upgrade(
                &Snapshot::from_state(&self.state.authoritative),
                request.item_id,
            );
            let Some(vote) = wow_policy::economy::loot::group_loot_vote(
                method,
                Some(&policy_request),
                elapsed,
                LootEligibility::Eligible,
                usable_upgrade,
                quality,
                loot_policy,
            ) else {
                continue;
            };
            let choice = match vote {
                LootRollVote::Pass => LootRollChoice::Pass,
                LootRollVote::Need => LootRollChoice::Need,
                LootRollVote::Greed => LootRollChoice::Greed,
                LootRollVote::Disenchant => LootRollChoice::Disenchant,
            };
            let command = GameplayCommand::LootRollVote {
                item: request.item,
                item_slot: request.item_slot,
                choice,
            };
            self.last_dispatch = DispatchOutcome::Rejected;
            if !self.propose_command(command, false).await {
                return false;
            }
            if self.last_dispatch == DispatchOutcome::Sent {
                self.group_loot_votes_sent.insert(request.item);
                tracing::info!(lane=?self.state.lane, item_id=request.item_id, item_slot=request.item_slot, ?choice, "submitted a conservative vote for a fresh server loot roll");
                return true;
            }
        }
        false
    }

    fn battleground_pvp_authorized(&self) -> bool {
        matches!(
            &self.state.mission.intent,
            MissionIntent::Battleground { .. }
        ) && self
            .state
            .mission
            .permissions
            .contains(PermissionSet::COMBAT)
    }

    async fn tick_battleground_combat(&mut self) -> bool {
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        if !self.battleground_pvp_authorized()
            || self.player_life_status() != PlayerLifeStatus::Alive
        {
            self.waiting(
                "battleground combat is waiting for mission authority and a living player".into(),
            );
            return true;
        }
        let Some(target) =
            wow_policy::battleground::combat::select_hostile_player_target(&snapshot)
        else {
            self.waiting("battleground match is active; waiting for an observed hostile player on this battleground map".into());
            return true;
        };
        let selected = match wow_policy::combat::selector::select_action(&snapshot, target) {
            Ok(selected) => selected,
            Err(reason) => {
                self.waiting(format!(
                    "battleground combat against {target} deferred: {reason}"
                ));
                return true;
            }
        };
        let pending = self.combat_pending(target, selected.cycle);
        self.last_dispatch = DispatchOutcome::Rejected;
        if !self.propose_command(selected.command, false).await {
            return false;
        }
        match self.last_dispatch {
            DispatchOutcome::Sent => self.pending_quest_action = Some(pending),
            DispatchOutcome::DeferredMovement => {
                if let Some(movement) = self.pending_movement.as_mut() {
                    movement.resume_pending = Some(pending);
                }
            }
            DispatchOutcome::DeferredDismount => {
                if let Some(dismount) = self.pending_dismount.as_mut() {
                    dismount.quest_action = Some(pending);
                }
            }
            _ => {}
        }
        true
    }

    async fn tick_goal(&mut self, text: &str) -> bool {
        if self.state.authoritative.quests.active.is_empty() {
            self.waiting(format!(
                "Goal mission {text:?} is waiting for grounded supported work; no active quest state is available"
            ));
            return true;
        }
        if !self.state.authoritative.quests.offers.is_empty() {
            self.waiting(format!(
                "Goal mission {text:?} is waiting because quest offers are outside its grounded active work"
            ));
            return true;
        }
        // Goal work can safely reuse the supported quest task flow while an
        // authoritative quest is active. Free-form Goal text alone cannot name
        // a target or authorize an action.
        self.tick_quest().await
    }

    async fn tick_group_encounter(&mut self) -> bool {
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        let player = self
            .state
            .authoritative
            .session
            .character_guid
            .map(EntityId);
        let has_observed_group_member =
            wow_policy::group::state::online_members_except(&snapshot, player)
                .next()
                .is_some();
        if !has_observed_group_member {
            self.group_pull_delay = None;
            self.waiting("group mission is waiting for an observed online group member".into());
            return true;
        }

        let Some(target) = wow_policy::group::encounter::from_observed(&snapshot).preferred_target
        else {
            self.group_pull_delay = None;
            return self.tick_group_follow(&snapshot, player).await;
        };
        let Some(target_state) = self.state.authoritative.entities.0.get(&target) else {
            self.group_pull_delay = None;
            self.waiting(
                "observed group encounter target is not present in current entity state".into(),
            );
            return true;
        };
        if !target_state.hostile || target_state.is_dead() {
            self.group_pull_delay = None;
            self.waiting("observed group encounter target is not a live hostile".into());
            return true;
        }

        let role = match &self.state.mission.intent {
            MissionIntent::Party { role } | MissionIntent::Raid { role } => *role,
            _ => GroupRole::Auto,
        };
        let threat_delay_elapsed =
            self.group_pull_delay_elapsed(&snapshot, role, player, target, Instant::now());
        match wow_policy::group::encounter::pull_decision(
            &snapshot,
            role,
            player,
            target,
            threat_delay_elapsed,
        ) {
            wow_policy::group::encounter::PullDecision::WaitForGroupEngagement => {
                self.waiting("group role is waiting for an observed authorized pull".into());
                return true;
            }
            wow_policy::group::encounter::PullDecision::WaitForThreatDelay => {
                self.waiting(
                    "group role is waiting for the configured threat-establishment delay".into(),
                );
                return true;
            }
            wow_policy::group::encounter::PullDecision::Engage => {}
        }

        self.dispatch_combat_target(target, false).await
    }

    fn group_pull_delay_elapsed(
        &mut self,
        snapshot: &Snapshot,
        role: GroupRole,
        player: Option<EntityId>,
        target: EntityId,
        now: Instant,
    ) -> bool {
        if role == GroupRole::Tank {
            self.group_pull_delay = None;
            return true;
        }
        if !wow_policy::group::encounter::is_group_engaged(snapshot, player, target) {
            self.group_pull_delay = None;
            return false;
        }
        let generation = snapshot.state.group.generation;
        let map = snapshot.state.position.player.map(|position| position.map);
        let started_at = match self.group_pull_delay {
            Some(delay)
                if delay.target == target
                    && delay.group_generation == generation
                    && delay.map == map =>
            {
                delay.started_at
            }
            _ => {
                self.group_pull_delay = Some(GroupPullDelay {
                    target,
                    group_generation: generation,
                    map,
                    started_at: now,
                });
                now
            }
        };
        now.duration_since(started_at)
            >= Duration::from_millis(self.runtime_tuning.group.threat_delay_ms())
    }

    async fn tick_group_follow(&mut self, snapshot: &Snapshot, player: Option<EntityId>) -> bool {
        let Some(player) = player else {
            self.waiting("group follow is waiting for player GUID".into());
            return true;
        };
        let Some(member) = wow_policy::group::follow::target_position(snapshot, player) else {
            self.waiting("group follow is waiting for an observed online member position".into());
            return true;
        };
        let Some(from) = self
            .state
            .authoritative
            .control
            .active_position(self.state.authoritative.position.player)
        else {
            self.waiting("group follow is waiting for authoritative player position".into());
            return true;
        };
        let Some(destination) = wow_policy::group::follow::follow_destination(
            from,
            member,
            group_follow_stop_distance(&self.state.mission.intent),
        ) else {
            return true;
        };
        let Some(controller) = self.movement_controller.as_ref() else {
            self.waiting("group follow requires navigation controller".into());
            return true;
        };
        let controlled_mover = self.state.authoritative.control.mover.is_some();
        let flags = if controlled_mover {
            self.state.authoritative.control.movement_flags
        } else {
            self.state.authoritative.position.flags
        };
        let mode = wow_navigation::LocomotionMode::from_server_flags(controlled_mover, flags);
        match controller.next_step(from, destination, 1.0, mode) {
            Ok(Some(step)) => {
                self.propose_command(GameplayCommand::MoveTo(step.next), false)
                    .await
            }
            Ok(None) => true,
            Err(error) => {
                self.waiting(format!("group follow navigation failed: {error:?}"));
                true
            }
        }
    }

    async fn tick_survival(&mut self, target: EntityId) -> bool {
        if self.last_survival_action.is_some_and(|(previous, at)| {
            previous == target && at.elapsed() < Duration::from_millis(2750)
        }) {
            return true;
        }
        self.dispatch_combat_target(target, true).await
    }

    fn player_life_status(&self) -> PlayerLifeStatus {
        let Some(player) = self
            .state
            .authoritative
            .session
            .character_guid
            .map(EntityId)
        else {
            return PlayerLifeStatus::Unknown;
        };
        if self
            .state
            .authoritative
            .auras
            .has_any(player, &[wow_state::life::GHOST_AURA_SPELL_ID])
        {
            return PlayerLifeStatus::Ghost;
        }
        match self
            .state
            .authoritative
            .entities
            .0
            .get(&player)
            .and_then(|entity| entity.health)
        {
            Some((0, _)) => PlayerLifeStatus::Dead,
            Some(_) => PlayerLifeStatus::Alive,
            None => PlayerLifeStatus::Unknown,
        }
    }

    fn player_is_dead(&self) -> bool {
        self.player_life_status().requires_recovery()
    }

    async fn tick_death_recovery(&mut self) -> bool {
        let generation = self.state.authoritative.life.recovery_generation;
        if generation != self.corpse_recovery_generation {
            self.corpse_recovery_generation = generation;
            self.corpse_reclaim_attempts = 0;
        }
        let now = Instant::now();
        if self
            .last_recovery_action
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return true;
        }
        self.last_recovery_action = Some(now);
        self.pending_quest_action = None;
        self.post_combat_loot = None;
        if self
            .stop_active_movement_for_handoff("death recovery took control")
            .await
        {
            return true;
        }
        let Some(player_guid) = self
            .state
            .authoritative
            .session
            .character_guid
            .map(EntityId)
        else {
            self.waiting("death recovery waiting for player GUID".into());
            return true;
        };
        let Some(player_pos) = self
            .state
            .authoritative
            .control
            .active_position(self.state.authoritative.position.player)
        else {
            self.waiting("death recovery waiting for authoritative position".into());
            return true;
        };
        let wall = Millis::wall_clock_now().0;
        let Some(corpse) = self.state.authoritative.life.corpse else {
            tracing::info!(lane=?self.state.lane,"death recovery releasing spirit and querying corpse");
            let _ = self.propose_recovery(GameplayCommand::ReleaseSpirit).await;
            return self.propose_recovery(GameplayCommand::QueryCorpse).await;
        };
        if !corpse_route_map_matches(player_pos, corpse) {
            self.waiting(format!("death recovery has no observed entrance route from map {} to corpse map {}; waiting for a grounded map transition",player_pos.map,corpse.map));
            return true;
        }
        let distance = player_pos.point.distance(corpse.point);
        if distance > 4.5 {
            let mode = wow_navigation::LocomotionMode::from_server_flags(
                self.state.authoritative.control.mover.is_some(),
                self.state.authoritative.control.movement_flags,
            );
            let Some(controller) = self.movement_controller.as_ref() else {
                self.waiting("death recovery requires navigation controller".into());
                return true;
            };
            match controller.next_step(player_pos, corpse.point, 4.0, mode) {
                Ok(Some(step)) => {
                    tracing::info!(lane=?self.state.lane,remaining=step.remaining,"death recovery moving ghost toward corpse");
                    return self
                        .propose_recovery(GameplayCommand::MoveTo(step.next))
                        .await;
                }
                Ok(None) => {}
                Err(error) => {
                    self.waiting(format!("death recovery navigation failed: {error:?}"));
                    return true;
                }
            }
        }
        if wall < self.state.authoritative.life.reclaim_ready_at_ms {
            self.waiting("death recovery waiting for corpse reclaim timer".into());
            return true;
        }
        if !corpse_reclaim_is_safe(&self.state.authoritative.entities.0, player_guid, corpse) {
            self.waiting(
                "death recovery waiting for nearby hostiles to leave corpse reclaim area".into(),
            );
            return true;
        }
        if !corpse_reclaim_attempt_allowed(self.corpse_reclaim_attempts) {
            self.waiting("death recovery stopped after bounded corpse reclaim attempts; waiting for new corpse state or resurrection".into());
            return true;
        }
        self.corpse_reclaim_attempts = self.corpse_reclaim_attempts.saturating_add(1);
        tracing::info!(lane=?self.state.lane,?player_guid,"death recovery reclaiming corpse");
        self.propose_recovery(GameplayCommand::ReclaimCorpse {
            player: player_guid,
        })
        .await
    }

    async fn stop_active_movement_for_handoff(&mut self, reason: &'static str) -> bool {
        if self.pending_movement.is_none() {
            return false;
        }
        tracing::info!(lane=?self.state.lane, reason, "stopped active movement for control handoff");
        self.cancel_route_job();
        self.pending_movement = None;
        self.pending_facing = None;
        self.current_work = None;
        self.propose_recovery(GameplayCommand::StopMovement).await;
        true
    }

    async fn propose_recovery(&mut self, command: GameplayCommand) -> bool {
        let action = ProposedAction {
            id: ActionId(self.next_action),
            task: TaskId(self.next_task),
            origin: PlanOrigin::Recovery,
            stamp: self.state.stamp(),
            command,
        };
        self.next_action = self.next_action.wrapping_add(1).max(1);
        self.next_task = self.next_task.wrapping_add(1).max(1);
        self.submit(action).await
    }

    fn maintenance_eligible(&self) -> bool {
        if self.pending_quest_action.is_some()
            || self.pending_accept.is_some()
            || self.pending_turn_in.is_some()
        {
            return false;
        }
        if self.state.authoritative.control.mover.is_some()
            || self.state.authoritative.position.moving
        {
            return false;
        }
        if self
            .last_quest_step
            .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
        {
            return false;
        }
        self.last_maintenance_tick
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(2))
    }

    fn tick_active_player_recovery(&mut self, snapshot: &Snapshot, now: Instant) -> bool {
        let Some(mut active) = self.active_player_recovery.take() else {
            return false;
        };
        let player_state = snapshot.state.entities.0.get(&active.selection.player);
        let current_resource = player_state.and_then(|player| match active.selection.kind {
            wow_policy::maintenance::PlayerRecoveryKind::Food => {
                player.health.map(|(current, _)| current)
            }
            wow_policy::maintenance::PlayerRecoveryKind::Drink => {
                player.power.map(|(current, _)| current)
            }
        });
        let unsafe_state = !snapshot.state.session.in_world
            || snapshot.state.control.mover.is_some()
            || snapshot.state.position.moving
            || snapshot.state.transport.attached == Some(true)
            || self.pending_movement.is_some()
            || player_state
                .is_none_or(|player| player.is_dead() || player.in_combat() != Some(false));
        if unsafe_state {
            self.maintenance_retry_after.insert(
                (active.selection.spell, active.selection.player),
                now + PLAYER_RECOVERY_RETRY,
            );
            self.last_maintenance_status = Some("player_recovery_preempted".into());
            tracing::info!(lane=?self.state.lane, kind=?active.selection.kind, reason="combat, control, or movement", "player food or drink recovery released its lane lease");
            return false;
        }

        let Some(current_resource) = current_resource else {
            self.maintenance_retry_after.insert(
                (active.selection.spell, active.selection.player),
                now + PLAYER_RECOVERY_RETRY,
            );
            return false;
        };
        let maximum = player_state
            .and_then(|player| match active.selection.kind {
                wow_policy::maintenance::PlayerRecoveryKind::Food => player.health,
                wow_policy::maintenance::PlayerRecoveryKind::Drink => player.power,
            })
            .map(|(_, maximum)| maximum)
            .unwrap_or_default();
        let aura_present = snapshot
            .state
            .auras
            .by_entity
            .get(&active.selection.player)
            .is_some_and(|auras| {
                auras
                    .values()
                    .any(|aura| aura.spell == active.selection.spell)
            });
        active.aura_seen |= aura_present;
        if wow_policy::maintenance::player_recovery_target_reached(
            active.selection.kind,
            current_resource,
            maximum,
        ) {
            self.maintenance_retry_after
                .remove(&(active.selection.spell, active.selection.player));
            self.last_maintenance_status = Some("player_recovery_complete".into());
            tracing::info!(lane=?self.state.lane, kind=?active.selection.kind, "authoritative health or mana reached the player recovery target");
            return false;
        }

        if current_resource > active.last_resource {
            active.last_resource = current_resource;
            active.last_progress_at = now;
        }
        let aura_stopped = active.aura_seen && !aura_present;
        let stalled = now.saturating_duration_since(active.started_at)
            >= PLAYER_RECOVERY_START_GRACE
            && now.saturating_duration_since(active.last_progress_at) >= PLAYER_RECOVERY_STALL;
        if aura_stopped || now >= active.deadline || stalled {
            self.maintenance_retry_after.insert(
                (active.selection.spell, active.selection.player),
                now + PLAYER_RECOVERY_RETRY,
            );
            self.last_maintenance_status = Some("player_recovery_retry_wait".into());
            tracing::info!(lane=?self.state.lane, kind=?active.selection.kind, aura_stopped, stalled, timed_out=now >= active.deadline, "player food or drink recovery stopped before its target; retry delayed");
            return false;
        }

        self.active_player_recovery = Some(active);
        true
    }

    fn class_trainer_travel_allowed(&self, snapshot: &Snapshot) -> bool {
        if !self.class_training_due
            || self.state.authoritative.group.lifecycle != wow_state::group::GroupLifecycle::Solo
            || matches!(
                self.state.mission.intent,
                MissionIntent::Party { .. } | MissionIntent::Raid { .. }
            )
            || !self
                .state
                .mission
                .permissions
                .contains(PermissionSet::MAINTENANCE | PermissionSet::MOVE)
            || snapshot.state.inventory.money <= CLASS_TRAINER_MONEY_RESERVE_COPPER
            || wow_policy::combat::engagement::survival_attacker(snapshot).is_some()
        {
            return false;
        }
        let Some(position) = snapshot
            .state
            .control
            .active_position(snapshot.state.position.player)
        else {
            return false;
        };
        let player_in_combat = snapshot
            .state
            .session
            .character_guid
            .map(EntityId)
            .and_then(|player| snapshot.state.entities.0.get(&player))
            .and_then(wow_state::entities::EntityState::in_combat)
            .unwrap_or(false);
        !player_in_combat && !wow_policy::maintenance::nearby_quest_offer(snapshot, position)
    }

    fn profession_trainer_travel_allowed(&self, snapshot: &Snapshot) -> bool {
        self.profession_training_service_allowed(snapshot)
            && wow_policy::gathering::professions::actionable_missing_skills(snapshot)
                .is_some_and(|skills| !skills.is_empty())
    }

    fn profession_training_service_allowed(&self, snapshot: &Snapshot) -> bool {
        if !self.runtime_tuning.maintenance.auto_professions_enabled
            || !wow_policy::gathering::professions::bootstrap_due(snapshot)
            || matches!(
                self.state.mission.intent,
                MissionIntent::Party { .. } | MissionIntent::Raid { .. }
            )
            || !self
                .state
                .mission
                .permissions
                .contains(PermissionSet::MAINTENANCE | PermissionSet::MOVE)
            || snapshot.state.inventory.money <= CLASS_TRAINER_MONEY_RESERVE_COPPER
            || !wow_policy::gathering::professions::training_state_safe(snapshot)
        {
            return false;
        }
        true
    }

    async fn tick_profession_training(
        &mut self,
        snapshot: &Snapshot,
        now: Instant,
    ) -> Option<bool> {
        if !self.profession_training_service_allowed(snapshot) {
            return None;
        }
        match wow_policy::gathering::professions::nearby_training_decision(
            snapshot,
            &self.maintenance_retry_after,
            now,
        )? {
            wow_policy::maintenance::MaintenanceDecision::TrainerList { trainer } => {
                self.last_maintenance_status =
                    Some(format!("profession_trainer_list:{}", trainer.0));
                self.maintenance_retry_after
                    .insert((0, trainer), now + Duration::from_secs(10));
                Some(
                    self.propose_command(GameplayCommand::TrainerList { trainer }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::TrainerBuy { trainer, spell } => {
                self.last_maintenance_status =
                    Some(format!("profession_trainer_buy:{spell}:{}", trainer.0));
                self.maintenance_retry_after
                    .insert((spell, trainer), now + Duration::from_secs(10));
                self.maintenance_retry_after
                    .insert((u32::MAX, trainer), now + Duration::from_secs(10));
                Some(
                    self.propose_command(GameplayCommand::TrainerBuy { trainer, spell }, false)
                        .await,
                )
            }
            _ => None,
        }
    }

    fn tick_profession_trainer_travel(&mut self, snapshot: &Snapshot) -> Option<bool> {
        if !self.profession_trainer_travel_allowed(snapshot)
            || nearby_profession_trainer(snapshot).is_some()
            || snapshot.state.control.mover.is_some()
            || snapshot.state.position.moving
            || self
                .profession_trainer_travel_retry_after
                .is_some_and(|deadline| deadline > Instant::now())
        {
            return None;
        }
        let position = snapshot
            .state
            .control
            .active_position(snapshot.state.position.player)?;
        let (_, destination) = wow_policy::gathering::professions::trainer_destination(
            snapshot,
            wow_infra::world_knowledge::embedded_azerothcore_catalog(),
        )?;
        if destination.map != position.map
            || destination.point.distance(position.point) > PROFESSION_TRAINER_DETOUR_RADIUS_YARDS
            || destination.point.distance(position.point) <= 5.0
        {
            return None;
        }
        self.profession_trainer_travel_retry_after =
            Some(Instant::now() + PROFESSION_TRAINER_TRAVEL_RETRY);
        let work = self.set_work(QuestWorkKey::TravelToObjective {
            quest: 0,
            objective: 0,
            destination,
        });
        self.queue_world_movement(
            destination,
            5.0,
            None,
            None,
            PlanOrigin::SystemPolicy,
            work,
            MovementPurpose::ProfessionTrainer,
        );
        Some(true)
    }

    fn tick_class_trainer_travel(&mut self, snapshot: &Snapshot) -> Option<bool> {
        if !self.class_trainer_travel_allowed(snapshot)
            || nearby_class_trainer(snapshot).is_some()
            || self
                .class_trainer_travel_retry_after
                .is_some_and(|deadline| deadline > Instant::now())
        {
            return None;
        }
        let now = Instant::now();
        let Some(destination) =
            remembered_class_trainer_destination(snapshot, &self.remembered_class_trainers, now)
        else {
            return None;
        };
        let Some(position) = snapshot
            .state
            .control
            .active_position(snapshot.state.position.player)
        else {
            return None;
        };
        if destination.point.distance(position.point) <= 5.0 {
            self.class_trainer_travel_retry_after = Some(now + CLASS_TRAINER_TRAVEL_RETRY);
            return None;
        }
        self.class_trainer_travel_retry_after = Some(now + CLASS_TRAINER_TRAVEL_RETRY);
        let work = self.set_work(QuestWorkKey::TravelToObjective {
            quest: 0,
            objective: 0,
            destination,
        });
        self.queue_world_movement(
            destination,
            5.0,
            None,
            None,
            PlanOrigin::SystemPolicy,
            work,
            MovementPurpose::ClassTrainer,
        );
        tracing::info!(
            lane=?self.state.lane,
            map=destination.map,
            distance_yards=position.point.distance(destination.point),
            "class training selected a recent observed trainer for a short same-map detour"
        );
        Some(true)
    }

    async fn tick_mailbox(&mut self, snapshot: &Snapshot) -> Option<bool> {
        let now = Instant::now();
        let generation = snapshot.state.inventory.mailbox.generation;
        if let Some((pending_generation, deadline)) = self.mail_action_pending {
            if !waiting_for_mail_observation(pending_generation, generation, deadline, now) {
                if generation == pending_generation {
                    self.mail_action_pending = None;
                    self.mail_retry_after = Some(now + MAILBOX_RETRY_DELAY);
                    return None;
                }
                self.mail_action_pending = None;
                self.mail_retry_after = None;
            } else {
                self.waiting(
                    "waiting for authoritative mailbox update after mail collection".into(),
                );
                return Some(true);
            }
        }
        if self.mail_retry_after.is_some_and(|deadline| deadline > now) {
            return None;
        }
        let mailbox = crate::trusted::unique_nearby_mailbox(snapshot)?;
        if let Some((requested_mailbox, request_generation, deadline)) = self.mailbox_list_pending {
            if !waiting_for_mail_observation(request_generation, generation, deadline, now) {
                if generation == request_generation {
                    self.mailbox_list_pending = None;
                    self.mail_retry_after = Some(now + MAILBOX_RETRY_DELAY);
                    return None;
                }
                self.mailbox_source = Some(requested_mailbox);
                self.mailbox_list_pending = None;
            } else {
                self.waiting("waiting for authoritative mailbox contents".into());
                return Some(true);
            }
        }
        if !snapshot.state.inventory.mailbox.authoritative || self.mailbox_source != Some(mailbox) {
            self.mailbox_list_pending = Some((mailbox, generation, now + MAILBOX_ACTION_TIMEOUT));
            return Some(
                self.propose_command(GameplayCommand::MailboxList { mailbox }, false)
                    .await,
            );
        }
        for mail in snapshot.state.inventory.mailbox.mails.values() {
            if mail.cod_copper != Some(0) {
                continue;
            }
            let target = if mail.money > 0 {
                Some(MailTakeTarget::Money)
            } else {
                mail.attachments
                    .keys()
                    .next()
                    .copied()
                    .map(|low_guid| MailTakeTarget::Attachment { low_guid })
            };
            let Some(target) = target else { continue };
            if matches!(target, MailTakeTarget::Attachment { .. })
                && snapshot.state.inventory.free_slots == 0
            {
                continue;
            }
            self.mail_action_pending = Some((generation, now + MAILBOX_ACTION_TIMEOUT));
            return Some(
                self.propose_command(
                    GameplayCommand::MailTake {
                        mailbox,
                        mailbox_generation: generation,
                        mail_id: mail.mail_id,
                        target,
                    },
                    false,
                )
                .await,
            );
        }
        None
    }

    async fn tick_bank_deposit(&mut self, snapshot: &Snapshot, now: Instant) -> Option<bool> {
        if let Some((banker, item_guid, baseline_count, deadline)) = self.bank_deposit_pending {
            let current_count = snapshot
                .state
                .inventory
                .instances
                .get(&item_guid)
                .map_or(0, |instance| instance.count);
            if current_count < baseline_count {
                self.bank_deposit_pending = None;
                self.bank_retry_after = None;
                self.last_maintenance_status =
                    Some(format!("bank_deposit_confirmed:{}", item_guid.0));
            } else if now < deadline {
                self.waiting("waiting for authoritative backpack state after bank deposit".into());
                return Some(true);
            } else {
                self.bank_deposit_pending = None;
                self.bank_retry_after = Some(now + BANK_RETRY_DELAY);
                self.waiting("bank deposit has no inventory confirmation; retry is delayed".into());
                return Some(true);
            }
            let _ = banker;
        }
        if let Some((banker, deadline)) = self.bank_open_pending {
            if snapshot.state.inventory.bank.authoritative
                && snapshot.state.inventory.bank.banker == Some(banker)
            {
                self.bank_open_pending = None;
            } else if now < deadline {
                self.waiting("waiting for authoritative bank-open observation".into());
                return Some(true);
            } else {
                self.bank_open_pending = None;
                self.bank_retry_after = Some(now + BANK_RETRY_DELAY);
                self.waiting("bank did not open; retry is delayed".into());
                return Some(true);
            }
        }
        if !self.runtime_tuning.maintenance.auto_bank_deposit_enabled
            || !wow_policy::economy::bank::bag_pressure_requires_bank(snapshot)
            || self.bank_retry_after.is_some_and(|deadline| deadline > now)
        {
            return None;
        }
        let banker = wow_policy::economy::bank::trusted_nearby_bankers(snapshot)
            .into_iter()
            .next()?;
        let Some(candidate) = wow_policy::economy::bank::profession_material_deposit_candidates(
            snapshot,
            banker,
            &wow_policy::economy::bank::protected_item_id_set(
                &self.runtime_tuning.maintenance.bank_keep_item_ids,
            ),
        )
        .into_iter()
        .next() else {
            return None;
        };

        if !snapshot.state.inventory.bank.authoritative
            || snapshot.state.inventory.bank.banker != Some(banker)
        {
            self.bank_open_pending = Some((banker, now + BANK_ACTION_TIMEOUT));
            let sent = self
                .propose_command(GameplayCommand::BankActivate { banker }, false)
                .await;
            if !sent {
                self.bank_open_pending = None;
                self.bank_retry_after = Some(now + BANK_RETRY_DELAY);
            }
            return Some(sent);
        }

        let baseline_count = snapshot
            .state
            .inventory
            .instances
            .get(&candidate.item_guid)
            .map_or(0, |instance| instance.count);
        self.bank_deposit_pending = Some((
            banker,
            candidate.item_guid,
            baseline_count,
            now + BANK_ACTION_TIMEOUT,
        ));
        let sent = self
            .propose_command(
                GameplayCommand::BankDeposit {
                    banker,
                    item: candidate.item,
                    item_guid: candidate.item_guid,
                    backpack_slot: candidate.backpack_slot,
                },
                false,
            )
            .await;
        if !sent {
            self.bank_deposit_pending = None;
            self.bank_retry_after = Some(now + BANK_RETRY_DELAY);
        }
        Some(sent)
    }

    fn bank_travel_allowed(&self, snapshot: &Snapshot) -> bool {
        if !self.runtime_tuning.maintenance.auto_bank_deposit_enabled
            || !snapshot.state.session.in_world
            || !snapshot.state.inventory.instances_authoritative
            || snapshot.state.inventory.free_slots
                > wow_policy::economy::bank::BANK_BAG_PRESSURE_FREE_SLOTS
            || self.state.authoritative.group.lifecycle != wow_state::group::GroupLifecycle::Solo
            || matches!(
                self.state.mission.intent,
                MissionIntent::Party { .. } | MissionIntent::Raid { .. }
            )
            || !self
                .state
                .mission
                .permissions
                .contains(PermissionSet::MAINTENANCE | PermissionSet::MOVE)
            || wow_policy::combat::engagement::survival_attacker(snapshot).is_some()
            || self.state.authoritative.control.mover.is_some()
            || self.state.authoritative.transport.attached == Some(true)
        {
            return false;
        }
        let Some(player) = snapshot
            .state
            .session
            .character_guid
            .map(EntityId)
            .and_then(|player| snapshot.state.entities.0.get(&player))
        else {
            return false;
        };
        if !player.health.is_some_and(|(current, _)| current > 0)
            || player.in_combat() == Some(true)
            || snapshot.state.active_casts.contains_key(&player.id)
            || snapshot
                .state
                .auras
                .has_any(player.id, &[wow_state::life::GHOST_AURA_SPELL_ID])
        {
            return false;
        }
        let Some(position) = snapshot
            .state
            .control
            .active_position(snapshot.state.position.player)
        else {
            return false;
        };
        let keep_item_ids = wow_policy::economy::bank::protected_item_id_set(
            &self.runtime_tuning.maintenance.bank_keep_item_ids,
        );
        !wow_policy::maintenance::nearby_quest_offer(snapshot, position)
            && !wow_policy::economy::bank::profession_material_deposit_candidates(
                snapshot,
                EntityId(0),
                &keep_item_ids,
            )
            .is_empty()
    }

    fn tick_bank_travel(&mut self, snapshot: &Snapshot) -> Option<bool> {
        let now = Instant::now();
        if !self.bank_travel_allowed(snapshot)
            || !wow_policy::economy::bank::trusted_nearby_bankers(snapshot).is_empty()
            || self
                .bank_travel_retry_after
                .is_some_and(|deadline| deadline > now)
        {
            return None;
        }
        let Some(destination) = wow_policy::economy::bank::remembered_banker_destination(
            snapshot,
            &self.remembered_bankers,
            now,
            BANKER_MEMORY_MAX_AGE,
            BANKER_DETOUR_RADIUS_YARDS,
        ) else {
            return None;
        };
        let Some(position) = snapshot
            .state
            .control
            .active_position(snapshot.state.position.player)
        else {
            return None;
        };
        if destination.point.distance(position.point)
            <= wow_policy::economy::bank::BANK_INTERACTION_RANGE_YARDS
        {
            self.bank_travel_retry_after = Some(now + BANKER_TRAVEL_RETRY);
            return None;
        }
        self.bank_travel_retry_after = Some(now + BANKER_TRAVEL_RETRY);
        let work = self.set_work(QuestWorkKey::TravelToObjective {
            quest: 0,
            objective: 0,
            destination,
        });
        self.queue_world_movement(
            destination,
            wow_policy::economy::bank::BANK_INTERACTION_RANGE_YARDS,
            None,
            None,
            PlanOrigin::SystemPolicy,
            work,
            MovementPurpose::Banker,
        );
        tracing::info!(
            lane=?self.state.lane,
            map=destination.map,
            distance_yards=position.point.distance(destination.point),
            "bag pressure selected a recent same-map banker location for a short service detour"
        );
        Some(true)
    }

    async fn tick_maintenance(&mut self) -> Option<bool> {
        self.last_maintenance_tick = Some(Instant::now());
        let now = Instant::now();
        if let Some((item, baseline_count, deadline)) = self.recovery_vendor_buy_pending {
            let current_count = self
                .state
                .authoritative
                .inventory
                .items
                .get(&item)
                .copied()
                .unwrap_or_default();
            if recovery_vendor_buy_pending_done(baseline_count, current_count, deadline, now) {
                self.recovery_vendor_buy_pending = None;
            }
        }
        self.maintenance_retry_after
            .retain(|_, deadline| *deadline > now);
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        if let Some(selection) = wow_policy::maintenance::select_player_recovery_item(&snapshot)
            && !self
                .maintenance_retry_after
                .get(&(selection.spell, selection.player))
                .is_some_and(|deadline| *deadline > now)
        {
            let resource = snapshot
                .state
                .entities
                .0
                .get(&selection.player)
                .and_then(|player| match selection.kind {
                    wow_policy::maintenance::PlayerRecoveryKind::Food => player.health,
                    wow_policy::maintenance::PlayerRecoveryKind::Drink => player.power,
                });
            if let Some((current, maximum)) = resource {
                let status = format!(
                    "player_recovery:{:?}:{}:{}",
                    selection.kind, selection.item, selection.spell
                );
                self.last_maintenance_status = Some(status);
                let started_at = now;
                let sent = self
                    .propose_command(
                        GameplayCommand::UseItemInstance {
                            item: selection.item,
                            item_guid: selection.item_guid,
                            backpack_slot: selection.backpack_slot,
                            spell: selection.spell,
                            target: Some(selection.player),
                            cast_count: 0,
                        },
                        false,
                    )
                    .await;
                if sent {
                    self.active_player_recovery = Some(ActivePlayerRecovery {
                        selection: selection.clone(),
                        started_at,
                        deadline: started_at + PLAYER_RECOVERY_MAX,
                        last_progress_at: started_at,
                        last_resource: current,
                        aura_seen: false,
                    });
                    self.maintenance_retry_after.insert(
                        (selection.spell, selection.player),
                        started_at + PLAYER_RECOVERY_RETRY,
                    );
                    tracing::info!(lane=?self.state.lane, kind=?selection.kind, item=selection.item, spell=selection.spell, current, maximum, "player recovery selected authoritative food or drink instance");
                }
                return Some(sent);
            }
        }
        if let Some(result) = self.tick_bank_deposit(&snapshot, now).await {
            return Some(result);
        }
        if let Some(result) = self.tick_bank_travel(&snapshot) {
            return Some(result);
        }
        if let Some(result) = self.tick_profession_training(&snapshot, now).await {
            return Some(result);
        }
        let nearby_sell_vendor = nearby_sell_vendor(&snapshot);
        match wow_policy::maintenance::decide_next_with_nearby_services(
            &snapshot,
            &self.maintenance_retry_after,
            now,
            true,
            nearby_sell_vendor,
            nearby_class_trainer(&snapshot),
        ) {
            wow_policy::maintenance::MaintenanceDecision::Cast {
                family,
                spell,
                target,
            } => {
                let status = format!("cast:{family}:{spell}:{}", target.0);
                if self.last_maintenance_status.as_deref() != Some(&status) {
                    tracing::info!(lane=?self.state.lane, %family, spell, ?target, "buff maintenance selected missing authoritative aura family");
                    self.last_maintenance_status = Some(status);
                }
                let retry = if family == "mage_water" || family == "mage_food" {
                    now + Duration::from_secs(10)
                } else if family.starts_with("warlock_create_") {
                    now + Duration::from_secs(15)
                } else {
                    wow_policy::maintenance::retry_deadline(now)
                };
                self.maintenance_retry_after.insert((spell, target), retry);
                Some(
                    self.propose_command(GameplayCommand::MaintainBuff { spell, target }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::SummonPet { spell, player } => {
                let status = format!("summon_pet:{spell}:{}", player.0);
                if self.last_maintenance_status.as_deref() != Some(&status) {
                    tracing::info!(lane=?self.state.lane, spell, ?player, "class pet maintenance selected a summon");
                    self.last_maintenance_status = Some(status);
                }
                let retry = if self.state.authoritative.pet.control_known {
                    Duration::from_secs(15)
                } else {
                    Duration::from_secs(60)
                };
                self.maintenance_retry_after
                    .insert((spell, player), now + retry);
                Some(
                    self.propose_command(GameplayCommand::SummonPet { spell, player }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::PetReaction { pet, reaction } => {
                self.last_maintenance_status = Some(format!("pet_reaction:{}:{reaction}", pet.0));
                self.maintenance_retry_after
                    .insert((0, pet), now + Duration::from_secs(15));
                Some(
                    self.propose_command(GameplayCommand::PetSetReaction { pet, reaction }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::PetAutocast {
                pet,
                spell,
                enabled,
            } => {
                self.last_maintenance_status =
                    Some(format!("pet_autocast:{}:{spell}:{enabled}", pet.0));
                self.maintenance_retry_after
                    .insert((spell, pet), now + Duration::from_secs(15));
                Some(
                    self.propose_command(
                        GameplayCommand::PetSetAutocast {
                            pet,
                            spell,
                            enabled,
                        },
                        false,
                    )
                    .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::EquipItem {
                item,
                item_guid,
                destination_slot,
                player,
            } => {
                let status = format!("equip:{item}:{destination_slot}");
                if self.last_maintenance_status.as_deref() != Some(&status) {
                    tracing::info!(lane=?self.state.lane, item, ?item_guid, destination_slot, "gear maintenance selected an inventory upgrade");
                    self.last_maintenance_status = Some(status);
                }
                self.maintenance_retry_after
                    .insert((item, player), now + Duration::from_secs(15));
                Some(
                    self.propose_command(
                        GameplayCommand::EquipItem {
                            item_guid,
                            destination_slot,
                        },
                        false,
                    )
                    .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::UseItemInstance {
                item,
                item_guid,
                backpack_slot,
                spell,
                target,
            } => {
                let status = format!("use_item:{item}:{spell}:{}", target.0);
                if self.last_maintenance_status.as_deref() != Some(&status) {
                    tracing::info!(lane=?self.state.lane, item, ?item_guid, spell, ?target, "warlock maintenance selected Soulstone application");
                    self.last_maintenance_status = Some(status);
                }
                self.maintenance_retry_after
                    .insert((spell, target), now + Duration::from_secs(30));
                Some(
                    self.propose_command(
                        GameplayCommand::UseItemInstance {
                            item,
                            item_guid,
                            backpack_slot,
                            spell,
                            target: Some(target),
                            cast_count: 0,
                        },
                        false,
                    )
                    .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::CastOnItem { spell, item_guid } => {
                let status = format!("imbue:{spell}:{}", item_guid.0);
                if self.last_maintenance_status.as_deref() != Some(&status) {
                    tracing::info!(lane=?self.state.lane, spell, ?item_guid, "shaman maintenance selected a weapon imbue");
                    self.last_maintenance_status = Some(status);
                }
                self.maintenance_retry_after
                    .insert((spell, item_guid), now + Duration::from_secs(20));
                Some(
                    self.propose_command(GameplayCommand::CastOnItem { spell, item_guid }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::UseItemOnItem {
                item,
                item_guid,
                backpack_slot,
                spell,
                target_item_guid,
            } => {
                let status = format!("weapon_poison:{item}:{spell}:{}", target_item_guid.0);
                if self.last_maintenance_status.as_deref() != Some(&status) {
                    tracing::info!(lane=?self.state.lane, item, spell, ?item_guid, ?target_item_guid, "rogue maintenance selected a weapon poison");
                    self.last_maintenance_status = Some(status);
                }
                self.maintenance_retry_after
                    .insert((spell, target_item_guid), now + Duration::from_secs(30));
                Some(
                    self.propose_command(
                        GameplayCommand::UseItemOnItem {
                            item,
                            item_guid,
                            backpack_slot,
                            spell,
                            target_item_guid,
                        },
                        false,
                    )
                    .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::VendorList { vendor } => {
                self.last_maintenance_status = Some(format!("poison_vendor_list:{}", vendor.0));
                self.maintenance_retry_after
                    .insert((0, vendor), now + Duration::from_secs(10));
                Some(
                    self.propose_command(GameplayCommand::VendorList { vendor }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::VendorBuy {
                vendor,
                item,
                slot,
                lots,
            } => {
                self.last_maintenance_status =
                    Some(format!("poison_vendor_buy:{item}:{slot}:{}", vendor.0));
                self.maintenance_retry_after
                    .insert((u32::MAX, vendor), now + Duration::from_secs(10));
                Some(
                    self.propose_command(
                        GameplayCommand::VendorBuy {
                            vendor,
                            item,
                            slot,
                            count: lots,
                        },
                        false,
                    )
                    .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::GearVendorList { vendor } => {
                self.last_maintenance_status = Some(format!("gear_vendor_list:{}", vendor.0));
                self.maintenance_retry_after
                    .insert((0, vendor), now + Duration::from_secs(10));
                Some(
                    self.propose_command(GameplayCommand::VendorList { vendor }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::GearVendorBuy { vendor, item, slot } => {
                self.last_maintenance_status =
                    Some(format!("gear_vendor_buy:{item}:{slot}:{}", vendor.0));
                self.maintenance_retry_after
                    .insert((u32::MAX, vendor), now + Duration::from_secs(10));
                Some(
                    self.propose_command(
                        GameplayCommand::VendorBuy {
                            vendor,
                            item,
                            slot,
                            count: wow_domain::MAX_MAINTENANCE_VENDOR_BUY_LOTS,
                        },
                        false,
                    )
                    .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::RecoveryVendorBuy {
                vendor,
                item,
                slot,
                lots,
            } => {
                if self.recovery_vendor_buy_pending.is_some() {
                    return None;
                }
                self.last_maintenance_status =
                    Some(format!("recovery_vendor_buy:{item}:{slot}:{}", vendor.0));
                self.recovery_vendor_buy_pending = Some((
                    item,
                    self.state
                        .authoritative
                        .inventory
                        .items
                        .get(&item)
                        .copied()
                        .unwrap_or_default(),
                    now + RECOVERY_VENDOR_BUY_PENDING_TIMEOUT,
                ));
                self.maintenance_retry_after.insert(
                    (u32::MAX, vendor),
                    now + RECOVERY_VENDOR_BUY_PENDING_TIMEOUT,
                );
                Some(
                    self.propose_command(
                        GameplayCommand::VendorBuy {
                            vendor,
                            item,
                            slot,
                            count: lots,
                        },
                        false,
                    )
                    .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::SetAmmo { item } => {
                self.last_maintenance_status = Some(format!("set_ranged_ammo:{item}"));
                self.maintenance_retry_after.insert(
                    (
                        item,
                        snapshot
                            .state
                            .session
                            .character_guid
                            .map(EntityId)
                            .unwrap_or_default(),
                    ),
                    now + Duration::from_secs(3_600),
                );
                Some(
                    self.propose_command(GameplayCommand::SetAmmo { item }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::TrainerList { trainer } => {
                self.last_maintenance_status = Some(format!("class_trainer_list:{}", trainer.0));
                self.maintenance_retry_after
                    .insert((0, trainer), now + Duration::from_secs(10));
                Some(
                    self.propose_command(GameplayCommand::TrainerList { trainer }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::TrainerBuy { trainer, spell } => {
                self.last_maintenance_status =
                    Some(format!("class_trainer_buy:{spell}:{}", trainer.0));
                self.maintenance_retry_after
                    .insert((spell, trainer), now + Duration::from_secs(10));
                self.maintenance_retry_after
                    .insert((u32::MAX, trainer), now + Duration::from_secs(10));
                Some(
                    self.propose_command(GameplayCommand::TrainerBuy { trainer, spell }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::QueryItem { item } => {
                self.maintenance_retry_after
                    .insert((item, EntityId(0)), now + Duration::from_secs(10));
                Some(
                    self.propose_command(GameplayCommand::QueryItem { item }, false)
                        .await,
                )
            }
            wow_policy::maintenance::MaintenanceDecision::Deferred { family, reason } => {
                let status = format!("deferred:{family}:{reason}");
                if self.last_maintenance_status.as_deref() != Some(&status) {
                    tracing::info!(lane=?self.state.lane, %family, %reason, "buff maintenance deferred");
                    self.last_maintenance_status = Some(status);
                }
                None
            }
            wow_policy::maintenance::MaintenanceDecision::Satisfied => {
                if self.last_maintenance_status.as_deref() != Some("satisfied") {
                    tracing::info!(lane=?self.state.lane, "buff maintenance satisfied for all authoritative families");
                    self.last_maintenance_status = Some("satisfied".into());
                }
                None
            }
        }
    }

    async fn tick_equipment_repair(&mut self, snapshot: &Snapshot) -> Option<bool> {
        let condition = snapshot.state.inventory.equipment_condition;
        if let Some((baseline, started)) = self.repair_pending {
            let improved = condition.observed
                && (condition.broken_items < baseline.broken_items
                    || condition
                        .lowest_durability_percent
                        .zip(baseline.lowest_durability_percent)
                        .is_some_and(|(current, previous)| current > previous));
            if improved {
                self.repair_pending = None;
                self.repair_retry_after = None;
                self.last_maintenance_status = Some("equipment_repaired".into());
                return None;
            } else if !wow_policy::maintenance::equipment_needs_repair(condition) {
                self.repair_pending = None;
                self.repair_retry_after = None;
                return None;
            } else if started.elapsed() < Duration::from_secs(15) {
                self.waiting("waiting for authoritative equipment repair state".into());
                return Some(true);
            } else {
                self.repair_pending = None;
                self.repair_retry_after = Some(Instant::now() + Duration::from_secs(60));
                self.waiting(
                    "repair request has no authoritative durability update; retry is delayed"
                        .into(),
                );
                return Some(true);
            }
        }
        if !repair_detour_should_continue(condition, self.state.authoritative.group.lifecycle)
            || !self
                .state
                .mission
                .permissions
                .contains(PermissionSet::MAINTENANCE)
        {
            return None;
        }
        if self
            .repair_retry_after
            .is_some_and(|deadline| deadline > Instant::now())
        {
            return None;
        }
        self.repair_retry_after = None;
        let Some(position) = snapshot
            .state
            .control
            .active_position(snapshot.state.position.player)
        else {
            self.waiting(
                "equipment needs repair; waiting for authoritative player position".into(),
            );
            return Some(true);
        };
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let Some((vendor_entry, destination)) = catalog.nearest_vendor(
            wow_infra::world_knowledge::VendorKind::Repair,
            position.map,
            position.point,
        ) else {
            self.waiting(format!(
                "equipment needs repair; no known repair vendor is available on map {}",
                position.map
            ));
            return Some(true);
        };
        let vendor = snapshot
            .state
            .entities
            .0
            .values()
            .filter(|entity| {
                entity.entry == vendor_entry
                    && entity.kind == wow_state::entities::EntityKind::Unit
                    && entity.interactable
            })
            .filter_map(|entity| entity.position.map(|target| (entity, target)))
            .filter(|(_, target)| target.map == position.map)
            .min_by(|(_, left), (_, right)| {
                left.point
                    .distance(position.point)
                    .total_cmp(&right.point.distance(position.point))
            })
            .map(|(entity, target)| (entity.id, target));
        if let Some((vendor, vendor_position)) = vendor {
            if vendor_position.point.distance(position.point) > 5.0
                && !self.state.mission.permissions.contains(PermissionSet::MOVE)
            {
                self.waiting(
                    "equipment needs repair; mission does not allow travel to the repair vendor"
                        .into(),
                );
                return Some(true);
            }
            let command = GameplayCommand::RepairEquipment { vendor };
            let status = format!(
                "repair:{}:{}",
                vendor.0,
                condition.lowest_durability_percent.unwrap_or(0)
            );
            if self.last_maintenance_status.as_deref() != Some(&status) {
                tracing::info!(lane=?self.state.lane, ?vendor, lowest_durability_percent=?condition.lowest_durability_percent, broken_items=condition.broken_items, "equipment repair selected a currently observed repair vendor");
                self.last_maintenance_status = Some(status);
            }
            return Some(self.propose_command(command, false).await);
        }
        if position.point.distance(destination) <= 5.0 {
            self.waiting(format!(
                "equipment needs repair; waiting for repair vendor entry {vendor_entry} to become visible"
            ));
            return Some(true);
        }
        if !self.state.mission.permissions.contains(PermissionSet::MOVE) {
            self.waiting(
                "equipment needs repair; mission does not allow travel to a repair vendor".into(),
            );
            return Some(true);
        }
        let work = self.set_work(QuestWorkKey::TravelToObjective {
            quest: 0,
            objective: 0,
            destination: WorldPosition {
                map: position.map,
                point: destination,
                orientation: 0.0,
            },
        });
        self.queue_world_movement(
            WorldPosition {
                map: position.map,
                point: destination,
                orientation: 0.0,
            },
            5.0,
            None,
            None,
            PlanOrigin::SystemPolicy,
            work,
            MovementPurpose::RepairVendor,
        );
        tracing::info!(lane=?self.state.lane, entry=vendor_entry, x=destination.x, y=destination.y, z=destination.z, "equipment repair selected a trusted local repair vendor hint");
        Some(true)
    }

    fn queue_movement(
        &mut self,
        destination: Vec3,
        acceptable_range: f32,
        resume: Option<GameplayCommand>,
        resume_pending: Option<PendingQuestAction>,
        resume_origin: PlanOrigin,
        work: QuestWorkRuntime,
        purpose: MovementPurpose,
    ) {
        let map = self
            .state
            .authoritative
            .control
            .active_position(self.state.authoritative.position.player)
            .map(|position| position.map);
        let map_known = map.is_some();
        self.queue_world_movement(
            WorldPosition {
                map: map.unwrap_or_default(),
                point: destination,
                orientation: 0.0,
            },
            acceptable_range,
            resume,
            resume_pending,
            resume_origin,
            work,
            purpose,
        );
        if !map_known && let Some(movement) = self.pending_movement.as_mut() {
            movement.destination_map_known = false;
        }
    }

    fn queue_world_movement(
        &mut self,
        destination: WorldPosition,
        acceptable_range: f32,
        resume: Option<GameplayCommand>,
        resume_pending: Option<PendingQuestAction>,
        resume_origin: PlanOrigin,
        work: QuestWorkRuntime,
        purpose: MovementPurpose,
    ) {
        let now = Instant::now();
        if let Some(job) = &self.route_job {
            job.cancellation.cancel();
        }
        let movement_id = MovementId(self.next_movement);
        self.next_movement = self.next_movement.wrapping_add(1).max(1);
        let initial_position = self
            .state
            .authoritative
            .control
            .active_position(self.state.authoritative.position.player);
        let initial_source = if self.state.authoritative.control.mover.is_some() {
            Some("controlled_mover")
        } else {
            initial_position.map(|_| "player")
        };
        self.pending_movement = Some(PendingMovement {
            runtime: crate::movement::MovementRuntime {
                id: movement_id,
                owner: TaskId(work.id.0),
                epoch: self.state.movement_epoch,
                destination: WorldPosition {
                    map: destination.map,
                    point: destination.point,
                    orientation: destination.orientation,
                },
                route: None,
                waypoint: 0,
            },
            destination,
            alternate_destinations: Vec::new(),
            destination_map_known: true,
            acceptable_range,
            resume,
            resume_pending,
            resume_origin,
            work,
            last_step: None,
            purpose,
            started_at: now,
            last_progress_at: now,
            last_progress_position: initial_position.map(|position| position.point),
            last_progress_log: None,
            progress_source: initial_source,
            progress_revision: initial_position.map(|_| self.state.authoritative.revision),
            last_player_client_time: initial_position
                .map(|_| self.state.authoritative.position.client_time),
            last_position_update_at: now,
            route_failures: 0,
            transport: None,
        });
    }

    fn cancel_route_job(&self) {
        if let Some(job) = &self.route_job {
            job.cancellation.cancel();
        }
    }

    async fn queue_search_movement(&mut self, destination: Vec3, work: QuestWorkRuntime) {
        if let Some(pending) = self.pending_travel_preparation {
            if pending.destination != self.local_world_position(destination)
                || pending.work_id != work.id
            {
                self.pending_travel_preparation = None;
            } else if self.travel_ability_active(pending.kind, pending.spell) {
                tracing::info!(lane=?self.state.lane, spell=pending.spell, ?pending.kind, "authoritative travel ability activation observed");
                self.pending_travel_preparation = None;
            } else if Instant::now() < pending.deadline {
                self.waiting(format!(
                    "travel spell {} is waiting for authoritative activation",
                    pending.spell
                ));
                return;
            } else {
                tracing::warn!(lane=?self.state.lane, spell=pending.spell, ?pending.kind, "travel ability activation was not observed before timeout");
                self.pending_travel_preparation = None;
                self.travel_preparation_skipped =
                    Some((work.id, self.local_world_position(destination)));
            }
        }

        if self.pending_travel_preparation.is_none()
            && self.travel_preparation_skipped
                != Some((work.id, self.local_world_position(destination)))
            && let Some(action) = self.travel_action_for(destination)
            && self
                .propose_command(
                    GameplayCommand::Cast {
                        spell: action.spell(),
                        target: None,
                    },
                    true,
                )
                .await
        {
            self.pending_travel_preparation = Some(PendingTravelPreparation {
                kind: action.kind(),
                spell: action.spell(),
                destination: self.local_world_position(destination),
                work_id: work.id,
                deadline: Instant::now() + Duration::from_secs(5),
            });
            self.waiting(format!(
                "travel spell {} is waiting for authoritative activation",
                action.spell()
            ));
            return;
        }

        self.queue_world_movement(
            self.local_world_position(destination),
            QUEST_SEARCH_ARRIVAL_RANGE,
            None,
            None,
            PlanOrigin::SystemPolicy,
            work,
            MovementPurpose::SearchArea,
        );
    }

    fn local_world_position(&self, point: Vec3) -> WorldPosition {
        WorldPosition {
            map: self
                .active_mover_position()
                .map_or(0, |position| position.map),
            point,
            orientation: 0.0,
        }
    }

    fn active_mover_position(&self) -> Option<WorldPosition> {
        self.state
            .authoritative
            .control
            .active_position(self.state.authoritative.position.player)
    }

    fn travel_action_for(&self, destination: Vec3) -> Option<wow_policy::travel::TravelAction> {
        use wow_policy::travel::{
            TravelAbilityKind, TravelContext, ready_travel_ability, select_travel_action,
            surface_safety,
        };
        const TRAVEL_FORMS: [u32; 2] = [783, 2645]; // Travel Form, Ghost Wolf
        let player_id = self
            .state
            .authoritative
            .session
            .character_guid
            .map(EntityId)?;
        let player = self.state.authoritative.entities.0.get(&player_id)?;
        let position = player.position?;
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        let alive = player.health.map(|(health, _)| health > 0);
        let in_combat = player.in_combat();
        let mounted = player.mounted();
        let nav_surface = self
            .movement_controller
            .as_ref()
            .and_then(|controller| controller.surface_kind_at(position));
        let surface = surface_safety(player.movement_flags, nav_surface);
        let base = TravelContext {
            alive,
            in_combat,
            controlled: self.state.authoritative.control.mover.is_some(),
            mounted,
            surface,
            distance_yards: position.point.distance(destination),
            ready_ability: None,
        };
        let now_ms = Millis::wall_clock_now().0;

        for spell in TRAVEL_FORMS {
            if self
                .state
                .authoritative
                .auras
                .spells(player_id)
                .contains(&spell)
            {
                return None;
            }
            if let Some(ready) =
                ready_travel_ability(&snapshot, TravelAbilityKind::SpeedForm, spell, now_ms)
            {
                let mut context = base;
                context.ready_ability = Some(ready);
                if let Some(action) = select_travel_action(context, &self.runtime_tuning) {
                    return Some(action);
                }
            }
        }

        let mount = wow_policy::combat::spells::mount_spell_ids()
            .iter()
            .copied()
            .filter(|spell| self.state.authoritative.capabilities.spells.contains(spell))
            .find_map(|spell| {
                ready_travel_ability(&snapshot, TravelAbilityKind::Mount, spell, now_ms)
            });
        let mut context = base;
        context.ready_ability = mount;
        select_travel_action(context, &self.runtime_tuning)
    }

    fn travel_ability_active(
        &self,
        kind: wow_policy::travel::TravelAbilityKind,
        spell: u32,
    ) -> bool {
        let Some(player_id) = self
            .state
            .authoritative
            .session
            .character_guid
            .map(EntityId)
        else {
            return false;
        };
        let Some(player) = self.state.authoritative.entities.0.get(&player_id) else {
            return false;
        };
        match kind {
            wow_policy::travel::TravelAbilityKind::SpeedForm => self
                .state
                .authoritative
                .auras
                .spells(player_id)
                .contains(&spell),
            wow_policy::travel::TravelAbilityKind::Mount => {
                player.mount_display_id.is_some_and(|display| display != 0)
            }
        }
    }

    fn observe_movement_position(
        &mut self,
        source: &'static str,
        position: WorldPosition,
        client_time: Option<u32>,
        revision: StateRevision,
    ) {
        let Some(movement) = self.pending_movement.as_mut() else {
            return;
        };
        if source == "player"
            && let Some(client_time) = client_time
        {
            let is_newer = movement.last_player_client_time.is_none_or(|previous| {
                let difference = client_time.wrapping_sub(previous);
                difference != 0 && difference < 0x8000_0000
            });
            if !is_newer {
                return;
            }
            movement.last_player_client_time = Some(client_time);
        } else if movement
            .progress_revision
            .is_some_and(|previous| revision <= previous)
        {
            return;
        }
        movement.last_position_update_at = Instant::now();
        if movement
            .last_progress_position
            .is_none_or(|previous| previous.distance(position.point) >= 0.35)
        {
            movement.last_progress_position = Some(position.point);
            movement.last_progress_at = Instant::now();
        }
        movement.progress_source = Some(source);
        movement.progress_revision = Some(revision);
    }

    async fn tick_movement(&mut self) -> bool {
        let Some(mut movement) = self.pending_movement.take() else {
            return true;
        };
        if movement
            .last_step
            .is_some_and(|at| at.elapsed() < MOVEMENT_STEP_INTERVAL - MOVEMENT_STEP_EARLY_TOLERANCE)
        {
            self.pending_movement = Some(movement);
            return true;
        }
        let Some(player) = self
            .state
            .authoritative
            .control
            .active_position(self.state.authoritative.position.player)
        else {
            self.waiting(format!(
                "quest work {:?} is waiting for canonical active-mover position",
                movement.work.key
            ));
            self.pending_movement = Some(movement);
            return true;
        };
        if !movement.destination_map_known {
            movement.destination.map = player.map;
            movement.runtime.destination.map = player.map;
            movement.destination_map_known = true;
        }
        if let Some(GameplayCommand::Loot(target)) = movement.resume.as_ref() {
            let snapshot = self.snapshot_with_post_combat_corpse(
                Snapshot::from_state(&self.state.authoritative),
                *target,
            );
            if let Some((refreshed_destination, acceptable_range)) =
                current_loot_approach(&snapshot, *target)
            {
                if movement.destination.map != refreshed_destination.map
                    || movement
                        .destination
                        .point
                        .distance(refreshed_destination.point)
                        > 0.5
                {
                    movement.destination = refreshed_destination;
                    movement.runtime.destination = refreshed_destination;
                    movement.acceptable_range = acceptable_range;
                    movement.last_step = None;
                    self.cancel_route_job();
                    tracing::info!(lane=?self.state.lane, ?target, destination=?refreshed_destination, "refreshed corpse approach from current target position");
                }
            }
        }
        if player.map != movement.destination.map || movement.transport.is_some() {
            let Some(routes) = self.transport_routes.as_ref() else {
                self.waiting(format!(
                    "travel goal is on map {} while the player is on map {}; no validated transport catalog is available",
                    movement.destination.map, player.map
                ));
                self.pending_movement = Some(movement);
                return true;
            };
            if movement.transport.is_none() {
                let selected_leg = routes
                    .route_for_maps(player.map, movement.destination.map)
                    .and_then(|route| route.into_iter().next());
                let Some((entry, leg)) = selected_leg else {
                    self.waiting(format!(
                        "no visible transport and no validated leg route connects map {} to map {}",
                        player.map, movement.destination.map
                    ));
                    self.pending_movement = Some(movement);
                    return true;
                };
                let transport = nearest_visible_gameobject(
                    &self.state.authoritative.entities.0,
                    entry,
                    player.map,
                    player.point,
                )
                // A runtime transport object may enter visibility only near
                // the authored terminal. Approach the grounded stop first.
                .unwrap_or(EntityId(0));
                movement.transport = Some((
                    crate::movement::transport::TransportTraversal {
                        transport,
                        boarding_point: leg.boarding,
                        exit_point: leg.exit,
                        proximity: 3.5,
                        max_vertical_delta: 3.0,
                        max_observations: 1200,
                    },
                    crate::movement::transport::TransportProgress::default(),
                ));
            }
            let (mut traversal, mut progress) = movement
                .transport
                .expect("transport traversal was selected");
            if traversal.transport == EntityId(0) {
                let transport_entry = routes
                    .route_for_maps(player.map, movement.destination.map)
                    .and_then(|route| route.into_iter().next())
                    .map(|(entry, _)| entry);
                let observed_transport = self.state.authoritative.transport.transport;
                let matched_transport = observed_transport.and_then(|id| {
                    self.state
                        .authoritative
                        .entities
                        .0
                        .get(&id)
                        .filter(|entity| {
                            Some(entity.entry) == transport_entry
                                && entity.kind == wow_state::entities::EntityKind::GameObject
                        })
                        .map(|entity| entity.id)
                });
                let visible_transport = transport_entry.and_then(|entry| {
                    nearest_visible_gameobject(
                        &self.state.authoritative.entities.0,
                        entry,
                        player.map,
                        player.point,
                    )
                });
                if let Some(id) = matched_transport.or(visible_transport) {
                    traversal.transport = id;
                } else if progress.phase
                    == crate::movement::transport::TransportPhase::AwaitBoarding
                {
                    progress.observations = progress.observations.saturating_add(1);
                    if progress.observations >= traversal.max_observations {
                        progress.phase = crate::movement::transport::TransportPhase::Blocked;
                    }
                    movement.transport = Some((traversal, progress));
                    self.waiting("at the authored boarding point; waiting for the server to identify the transport gameobject".into());
                    self.pending_movement = Some(movement);
                    return true;
                }
            }
            let step = crate::movement::transport::advance_transport(
                traversal,
                progress,
                Some(player),
                &self.state.authoritative.transport,
            );
            movement.transport = Some((traversal, step.progress));
            use crate::movement::transport::TransportAction;
            match step.action {
                TransportAction::MoveToBoardingPoint | TransportAction::MoveToExitPoint => {
                    let waypoint = if step.action == TransportAction::MoveToBoardingPoint {
                        traversal.boarding_point
                    } else {
                        traversal.exit_point
                    };
                    let next = self
                        .movement_controller
                        .as_ref()
                        .and_then(|controller| {
                            controller
                                .next_step(
                                    player,
                                    waypoint.point,
                                    1.0,
                                    wow_navigation::LocomotionMode::Ground,
                                )
                                .ok()
                                .flatten()
                        })
                        .map(|step| step.next);
                    if let Some(next) = next {
                        movement.last_step = Some(Instant::now());
                        self.pending_movement = Some(movement);
                        return self
                            .propose_command(GameplayCommand::MoveTo(next), false)
                            .await;
                    }
                    self.waiting("transport waypoint has no safe local navigation step".into());
                }
                TransportAction::WaitForBoarding => self.waiting(
                    "waiting for authoritative attachment to the selected transport".into(),
                ),
                TransportAction::WaitForRide => self.waiting(
                    "riding transport; waiting for authoritative detachment at the authored exit"
                        .into(),
                ),
                TransportAction::Complete => {
                    movement.transport = None;
                    movement.started_at = Instant::now();
                    movement.last_progress_at = Instant::now();
                    if player.map != movement.destination.map {
                        self.waiting("transport leg completed; selecting the next validated leg toward the travel goal".into());
                    }
                }
                TransportAction::Blocked => {
                    self.waiting("transport traversal stopped because authoritative boarding, ride, exit, or waypoint checks failed".into());
                    movement.transport = None;
                }
            }
            self.pending_movement = Some(movement);
            return true;
        }
        movement.transport = None;
        let controlled_mover = self.state.authoritative.control.mover.is_some();
        let movement_flags = if controlled_mover {
            self.state.authoritative.control.movement_flags
        } else {
            self.state.authoritative.position.flags
        };
        let locomotion =
            wow_navigation::LocomotionMode::from_server_flags(controlled_mover, movement_flags);
        let use_3d_arrival = matches!(&movement.work.key, QuestWorkKey::TurnIn { .. })
            || matches!(movement.resume.as_ref(), Some(GameplayCommand::Loot(_)));
        let distance = match locomotion {
            wow_navigation::LocomotionMode::Ground if use_3d_arrival => {
                player.point.distance(movement.destination.point)
            }
            wow_navigation::LocomotionMode::Ground => (movement.destination.point.x
                - player.point.x)
                .hypot(movement.destination.point.y - player.point.y),
            wow_navigation::LocomotionMode::Flight => {
                player.point.distance(movement.destination.point)
            }
        };

        let movement_failure = movement_failure(
            movement.started_at.elapsed(),
            movement.last_progress_at.elapsed(),
        );
        if movement_failure == Some(MovementFailure::TimedOut) {
            tracing::warn!(lane=?self.state.lane, work_id=?movement.work.id, key=?movement.work.key, ?locomotion, remaining=distance, "movement operation timed out");
            self.stop_owned_movement(movement.purpose).await;
            self.current_work = None;
            self.waiting(format!(
                "movement for {:?} timed out; waiting for authoritative state before retry",
                movement.work.key
            ));
            return true;
        }

        if movement_failure == Some(MovementFailure::Stalled) {
            let reason = if movement.last_position_update_at.elapsed() >= Duration::from_secs(5) {
                "no_new_authoritative_position"
            } else {
                "newer_positions_show_no_displacement"
            };
            tracing::warn!(lane=?self.state.lane, work_id=?movement.work.id, movement_id=?movement.runtime.id, key=?movement.work.key, ?locomotion, remaining=distance, source=movement.progress_source, revision=?movement.progress_revision, reason, "movement made no authoritative progress; stopping owned movement for bounded recovery");
            self.diagnostic(
                DiagnosticStream::Navigation,
                "movement_progress_stalled",
                serde_json::json!({
                    "work_id": movement.work.id.0,
                    "movement_id": movement.runtime.id.0,
                    "source": movement.progress_source,
                    "revision": movement.progress_revision.map(StateRevision::get),
                    "reason": reason,
                }),
            );
            self.stop_owned_movement(movement.purpose).await;
            self.current_work = None;
            self.waiting(format!(
                "movement for {:?} stalled; scheduler will re-ground before retry",
                movement.work.key
            ));
            return true;
        }
        if movement
            .last_progress_log
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(2))
        {
            tracing::info!(lane=?self.state.lane, work_id=?movement.work.id, movement_id=?movement.runtime.id, key=?movement.work.key, ?locomotion, remaining=distance, mover=?self.state.authoritative.control.mover, source=movement.progress_source, revision=?movement.progress_revision, reason="newer_authoritative_position_tracking", "movement operation progress");
            self.diagnostic(
                DiagnosticStream::MovementHeartbeat,
                "movement_progress",
                serde_json::json!({
                    "work_id": movement.work.id.0,
                    "locomotion": format!("{locomotion:?}"),
                    "remaining": distance,
                    "mover_controlled": controlled_mover,
                    "source": movement.progress_source,
                    "revision": movement.progress_revision.map(StateRevision::get),
                    "reason": "newer_authoritative_position_tracking",
                }),
            );
            movement.last_progress_log = Some(Instant::now());
        }
        if distance <= movement.acceptable_range {
            tracing::info!(lane=?self.state.lane, work_id=?movement.work.id, remaining=distance, purpose=?movement.purpose, "owned movement work reached interaction envelope");
            let mounted = self.observed_mounted() == Some(true);
            if mounted || !self.observed_travel_forms().is_empty() {
                self.begin_travel_cleanup();
            }
            self.record_search_arrival(&movement);
            self.stop_owned_movement(movement.purpose).await;
            // Keep the quest focus after reaching a search point. Reaching a
            // hint is not a reason to abandon that quest: the next scheduler
            // pass must continue searching this objective until it finds a
            // live target or a movement failure clears the work.
            if !is_quest_search_work(&movement) {
                self.current_work = None;
            }
            if let Some(resume) = movement.resume.take() {
                let work_key = movement.work.key.clone();
                tracing::info!(lane=?self.state.lane, work_id=?movement.work.id, command=?resume, work=?work_key, "resuming quest action after movement");
                if movement.resume_origin == PlanOrigin::Recovery {
                    self.last_survival_action = Some((
                        crate::action::spatial::profile(&resume)
                            .map(|profile| profile.target)
                            .unwrap_or(EntityId(0)),
                        Instant::now(),
                    ));
                    return self.propose_recovery(resume).await;
                }
                if let Some(pending) = movement.resume_pending.take() {
                    return self.dispatch_quest_semantic(resume, pending).await;
                }
                return self.dispatch_resumed_quest_action(resume, &work_key).await;
            }
            return true;
        }
        if locomotion == wow_navigation::LocomotionMode::Ground
            && let Some(controller) = self.movement_controller.clone()
            && controller.has_navigation_data()
            && !controller.route_is_ready(player.map, movement.destination.point)
        {
            if let Some(job) = self.route_job.as_mut() {
                let is_current = job.token
                    == (crate::movement::ReplanToken {
                        movement: movement.runtime.id,
                        epoch: movement.runtime.epoch,
                    });
                if !is_current {
                    job.cancellation.cancel();
                }
                if !job.deadline_exceeded && job.started_at.elapsed() >= ROUTE_PLAN_DEADLINE {
                    job.deadline_exceeded = true;
                    job.cancellation.cancel();
                }
            }
            if self
                .route_job
                .as_ref()
                .is_some_and(|job| job.task.is_finished())
            {
                let job = self.route_job.take().expect("finished route job exists");
                let result = job.task.await;
                let current_stamp = self.state.stamp();
                let stamp = job.stamped.stamp;
                let authority_matches = stamp.mission == current_stamp.mission
                    && stamp.permission == current_stamp.permission
                    && stamp.worker == current_stamp.worker
                    && stamp.ownership == current_stamp.ownership
                    && stamp.movement == current_stamp.movement;
                let route_start_is_current = job.stamped.value.map == player.map
                    && job.stamped.value.point.distance(player.point) <= 2.5;
                let token_matches = job.token.movement == movement.runtime.id
                    && job.token.epoch == movement.runtime.epoch
                    && movement.runtime.epoch == self.state.movement_epoch;
                if token_matches
                    && authority_matches
                    && route_start_is_current
                    && !job.cancellation.is_cancelled()
                {
                    match result {
                        Ok(Ok((destination, plan))) => {
                            movement.destination.point = destination;
                            movement.runtime.destination.point = destination;
                            movement.alternate_destinations.clear();
                            let points = plan.points().to_vec();
                            let cost = points
                                .windows(2)
                                .map(|pair| pair[0].distance(pair[1]))
                                .sum();
                            let route = wow_navigation::Route { points, cost };
                            if movement
                                .runtime
                                .replace_route(route, self.state.movement_epoch)
                            {
                                controller.install_route(plan, player.point);
                                movement.route_failures = 0;
                                self.diagnostic(
                                    DiagnosticStream::Navigation,
                                    "movement_route_installed",
                                    serde_json::json!({
                                        "movement_id": movement.runtime.id.0,
                                        "epoch": movement.runtime.epoch.get(),
                                        "source_revision": stamp.state.get(),
                                    }),
                                );
                            }
                        }
                        Ok(Err(error)) => {
                            movement.route_failures = movement.route_failures.saturating_add(1);
                            self.diagnostic(
                                DiagnosticStream::Navigation,
                                "movement_route_rejected",
                                serde_json::json!({
                                    "movement_id": movement.runtime.id.0,
                                    "epoch": movement.runtime.epoch.get(),
                                    "attempt": movement.route_failures,
                                    "reason": format!("{error:?}"),
                                }),
                            );
                            if movement.route_failures >= 3 {
                                let deferred =
                                    self.record_unreachable_combat_approach(&movement, &error);
                                self.pending_movement = None;
                                self.current_work = None;
                                if deferred {
                                    self.last_wait_reason = None;
                                    return true;
                                }
                                self.waiting("movement route planning failed three times; scheduler will re-ground before retry".into());
                                return true;
                            }
                            movement.last_step = Some(Instant::now());
                            self.pending_movement = Some(movement);
                            return true;
                        }
                        Err(error) => {
                            tracing::warn!(lane=?self.state.lane, ?error, "movement route worker failed");
                            movement.route_failures = movement.route_failures.saturating_add(1);
                            if movement.route_failures >= 3 {
                                self.waiting("movement route worker failed three times; scheduler will re-ground before retry".into());
                                self.pending_movement = None;
                                self.current_work = None;
                                return true;
                            }
                            movement.last_step = Some(Instant::now());
                            self.pending_movement = Some(movement);
                            return true;
                        }
                    }
                } else {
                    if token_matches && job.deadline_exceeded {
                        movement.route_failures = movement.route_failures.saturating_add(1);
                        self.diagnostic(
                            DiagnosticStream::Navigation,
                            "movement_route_deadline_exceeded",
                            serde_json::json!({
                                "movement_id": movement.runtime.id.0,
                                "epoch": movement.runtime.epoch.get(),
                                "attempt": movement.route_failures,
                                "deadline_ms": ROUTE_PLAN_DEADLINE.as_millis(),
                            }),
                        );
                        if movement.route_failures >= 3 {
                            self.waiting("movement route planning exceeded its deadline three times; scheduler will re-ground before retry".into());
                            self.pending_movement = None;
                            self.current_work = None;
                            return true;
                        }
                        movement.last_step = Some(Instant::now());
                        self.pending_movement = Some(movement);
                        return true;
                    }
                    self.diagnostic(
                        DiagnosticStream::Navigation,
                        "movement_route_discarded_stale",
                        serde_json::json!({
                            "movement_id": job.token.movement.0,
                            "epoch": job.token.epoch.get(),
                            "reason": "movement_or_authority_changed",
                        }),
                    );
                }
            }
            if self.route_job.is_some() {
                self.pending_movement = Some(movement);
                return true;
            }
            let permit = match controller.try_acquire_route_planning_slot() {
                Ok(permit) => permit,
                Err(error) => {
                    self.diagnostic(
                        DiagnosticStream::Navigation,
                        "movement_route_admission_deferred",
                        serde_json::json!({
                            "movement_id": movement.runtime.id.0,
                            "reason": format!("{error:?}"),
                        }),
                    );
                    movement.last_step = Some(Instant::now());
                    self.pending_movement = Some(movement);
                    return true;
                }
            };
            let cancellation = crate::runtime::CancellationToken::new();
            let worker_token = cancellation.clone();
            let worker_controller = controller.clone();
            let route_start = player;
            let mut route_destinations = vec![movement.destination.point];
            route_destinations.extend(movement.alternate_destinations.iter().copied());
            let path_straightness = self.runtime_tuning.movement.path_straightness;
            let route_planning_permit = permit;
            let stamped = crate::runtime::Stamped {
                stamp: self.state.stamp(),
                value: route_start,
            };
            let task = tokio::task::spawn_blocking(move || {
                let _permit = route_planning_permit;
                let mut last_error = None;
                for destination in route_destinations {
                    match worker_controller.plan_route_cancellable_with_straightness(
                        route_start,
                        destination,
                        path_straightness,
                        &|| worker_token.is_cancelled(),
                    ) {
                        Ok(plan) => return Ok((destination, plan)),
                        Err(error) => last_error = Some(error),
                    }
                    if worker_token.is_cancelled() {
                        return Err(wow_navigation::NavigationError::RoutePlanningCancelled);
                    }
                }
                Err(last_error.unwrap_or(wow_navigation::NavigationError::RoutePlanningCancelled))
            });
            self.route_job = Some(RoutePlanJob {
                token: crate::movement::ReplanToken {
                    movement: movement.runtime.id,
                    epoch: movement.runtime.epoch,
                },
                stamped,
                cancellation,
                started_at: Instant::now(),
                deadline_exceeded: false,
                task,
            });
            self.pending_movement = Some(movement);
            return true;
        }
        let run_speed = self
            .state
            .authoritative
            .position
            .run_speed_yards_per_second
            .filter(|speed| speed.is_finite() && (0.1..=100.0).contains(speed))
            .unwrap_or(BASE_RUN_SPEED_YARDS_PER_SECOND);
        let maximum_step = movement_step_distance(run_speed, movement.last_step, Instant::now());
        let step_result = match &self.movement_controller {
            Some(controller) => controller
                .clone()
                .with_maximum_step(maximum_step)
                .next_step_with_3d_arrival(
                    player,
                    movement.destination.point,
                    movement.acceptable_range,
                    locomotion,
                    use_3d_arrival,
                ),
            None => Err(wow_navigation::NavigationError::MissingNavigationData),
        };
        let next = match step_result {
            Ok(Some(step)) => {
                self.diagnostic(
                    DiagnosticStream::Navigation,
                    "movement_step_selected",
                    serde_json::json!({
                        "work_id": movement.work.id.0,
                        "locomotion": format!("{locomotion:?}"),
                        "remaining": distance,
                        "run_speed_yards_per_second": run_speed,
                        "step_interval_ms": MOVEMENT_STEP_INTERVAL.as_millis(),
                        "purpose": format!("{:?}", movement.purpose),
                        "step_remaining": step.remaining,
                        "from": {
                            "x": player.point.x,
                            "y": player.point.y,
                            "z": player.point.z,
                        },
                        "to": {
                            "x": step.next.x,
                            "y": step.next.y,
                            "z": step.next.z,
                        },
                    }),
                );
                step.next
            }
            Ok(None) => {
                self.pending_movement = Some(movement);
                return true;
            }
            Err(error) => {
                let genuine_path_failure = is_terminal_navigation_failure(&error);
                if genuine_path_failure {
                    movement.route_failures = movement.route_failures.saturating_add(1);
                }
                self.diagnostic(
                    DiagnosticStream::Navigation,
                    "movement_step_rejected",
                    serde_json::json!({
                        "work_id": movement.work.id.0,
                        "locomotion": format!("{locomotion:?}"),
                        "purpose": format!("{:?}", movement.purpose),
                        "reason": format!("{error:?}"),
                        "attempt": movement.route_failures,
                    }),
                );
                if genuine_path_failure
                    && movement.purpose == MovementPurpose::SearchArea
                    && movement.route_failures >= 3
                {
                    self.record_failed_search_destination(&movement);
                    if matches!(&movement.work.key, QuestWorkKey::TurnIn { .. }) {
                        tracing::warn!(lane=?self.state.lane, work_id=?movement.work.id, key=?movement.work.key, x=movement.destination.point.x, y=movement.destination.point.y, attempt=movement.route_failures, reason=?error, "turn-in search hint has no route; waiting for an authoritative giver or a new position");
                    } else {
                        tracing::warn!(lane=?self.state.lane, work_id=?movement.work.id, key=?movement.work.key, x=movement.destination.point.x, y=movement.destination.point.y, attempt=movement.route_failures, reason=?error, "search destination path failed after three step attempts; trying another destination");
                    }
                    self.stop_owned_movement(movement.purpose).await;
                    self.clear_quest_search_focus(movement.work.id);
                    return true;
                }
                if genuine_path_failure
                    && movement.route_failures >= 3
                    && self.record_unreachable_combat_approach(&movement, &error)
                {
                    self.pending_movement = None;
                    self.current_work = None;
                    self.last_wait_reason = None;
                    return true;
                }
                self.waiting(format!("movement step rejected: {error:?}"));
                movement.last_step = Some(Instant::now());
                self.pending_movement = Some(movement);
                return true;
            }
        };
        movement.last_step = Some(Instant::now());
        tracing::debug!(lane=?self.state.lane, work_id=?movement.work.id, ?locomotion, remaining=distance, next_x=next.x, next_y=next.y, next_z=next.z, "quest movement progress");
        let survival = movement.purpose == MovementPurpose::SurvivalApproach;
        self.pending_movement = Some(movement);
        if survival {
            self.propose_recovery(GameplayCommand::MoveTo(next)).await
        } else {
            self.propose_command(GameplayCommand::MoveTo(next), false)
                .await
        }
    }

    async fn stop_owned_movement(&mut self, purpose: MovementPurpose) {
        if purpose == MovementPurpose::SurvivalApproach {
            let _ = self.propose_recovery(GameplayCommand::StopMovement).await;
        } else {
            let _ = self
                .propose_command(GameplayCommand::StopMovement, false)
                .await;
        }
    }

    fn record_unreachable_combat_approach(
        &mut self,
        movement: &PendingMovement,
        error: &wow_navigation::NavigationError,
    ) -> bool {
        if movement.purpose != MovementPurpose::ApproachGroundedTarget
            || !matches!(
                error,
                wow_navigation::NavigationError::NoRoute
                    | wow_navigation::NavigationError::FloorDiscontinuity
            )
        {
            return false;
        }
        let Some(target) = movement
            .resume
            .as_ref()
            .and_then(crate::action::spatial::profile)
            .map(|profile| profile.target)
        else {
            return false;
        };
        self.combat_target_retry_after
            .insert(target, Instant::now() + Duration::from_secs(30));
        tracing::warn!(
            lane=?self.state.lane,
            ?target,
            ?error,
            "all combat approach routes failed; target deferred while quest search continues"
        );
        true
    }

    fn next_incomplete_quest(&self) -> Option<u32> {
        let now = Instant::now();
        let active = &self.state.authoritative.quests.active;
        let focused_quest = self.current_work.as_ref().and_then(|work| match &work.key {
            QuestWorkKey::TravelToObjective { quest, .. }
            | QuestWorkKey::CollectItem { quest, .. } => active
                .get(quest)
                .filter(|progress| !progress.complete)
                .map(|_| *quest),
            _ => None,
        });
        if focused_quest.is_some() {
            return focused_quest;
        }
        let available = active
            .iter()
            .find(|(quest, progress)| {
                !progress.complete
                    && !self
                        .search_retry_after
                        .iter()
                        .any(|((retry_quest, _, _), retry_after)| {
                            retry_quest == *quest && *retry_after > now
                        })
            })
            .map(|(&quest, _)| quest);

        available.or_else(|| {
            active
                .iter()
                .find(|(_, progress)| !progress.complete)
                .map(|(&quest, _)| quest)
        })
    }

    async fn tick_quest(&mut self) -> bool {
        if self.pending_quest_action_blocks() {
            return true;
        }
        if let Some((target, completed_at, cached_target)) = self.post_combat_loot.clone() {
            if self.bag_full_loot_targets.contains(&target) {
                self.waiting(format!(
                    "post-combat corpse {target} is waiting for free inventory space"
                ));
                return true;
            }
            if self
                .loot_retry_after
                .get(&target)
                .is_some_and(|(_, retry_after)| {
                    retry_after.is_some_and(|deadline| deadline > Instant::now())
                })
            {
                self.waiting(format!(
                    "post-combat corpse {target} is waiting for its loot retry delay"
                ));
                return true;
            }
            let corpse_present = self
                .state
                .authoritative
                .entities
                .0
                .get(&target)
                .is_some_and(|entity| entity.health.is_none_or(|(current, _)| current == 0));
            if corpse_present {
                let baseline_generation = self.state.authoritative.inventory.bot_loot_generation;
                tracing::info!(lane=?self.state.lane, ?target, baseline_generation, "post-combat corpse loot selected before next quest target");
                return self
                    .dispatch_quest_semantic(
                        GameplayCommand::Loot(target),
                        PendingQuestAction::CorpseLoot {
                            target,
                            baseline_generation,
                            started: Instant::now(),
                        },
                    )
                    .await;
            }
            if completed_at.elapsed() < Duration::from_secs(2) {
                self.waiting(format!("combat target {target} is waiting for authoritative corpse state before the next target"));
                return true;
            }
            if let Some(mut cached_target) = cached_target {
                cached_target.mark_dead();
                self.post_combat_loot = Some((target, completed_at, Some(cached_target)));
                let baseline_generation = self.state.authoritative.inventory.bot_loot_generation;
                tracing::info!(lane=?self.state.lane, ?target, baseline_generation, "post-combat loot attempt using last observed target while corpse update is delayed");
                return self
                    .dispatch_quest_semantic(
                        GameplayCommand::Loot(target),
                        PendingQuestAction::CorpseLoot {
                            target,
                            baseline_generation,
                            started: Instant::now(),
                        },
                    )
                    .await;
            }
            self.post_combat_loot = None;
        }
        if self
            .last_quest_step
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return true;
        }
        if let Some((quest, giver, at)) = self.pending_accept {
            if quest_confirmation_waiting(at, Instant::now()) {
                self.waiting(format!(
                    "awaiting server confirmation for quest {quest} from giver {giver}"
                ));
                return true;
            }
            tracing::warn!(lane=?self.state.lane, quest, ?giver, "quest accept confirmation timed out; allowing bounded retry");
            self.pending_accept = None;
        }
        if let Some((quest, at, step)) = self.pending_turn_in {
            if quest_confirmation_waiting(at, Instant::now()) {
                self.waiting(format!("awaiting authoritative server confirmation for quest {quest} turn-in step {step}"));
                return true;
            }
            tracing::warn!(lane=?self.state.lane, quest, step, "quest turn-in confirmation timed out; allowing bounded retry");
            self.pending_turn_in = None;
        }
        if let Some((giver, at)) = self.pending_giver_interaction {
            if quest_confirmation_waiting(at, Instant::now()) {
                self.waiting(format!(
                    "quest giver {giver} interaction is awaiting an authoritative quest list"
                ));
                return true;
            }
            self.defer_quest_giver(giver);
            tracing::warn!(lane=?self.state.lane, ?giver, "quest giver did not return a quest list; delaying another interaction");
            self.pending_giver_interaction = None;
        }

        let offer = self
            .state
            .authoritative
            .quests
            .offers
            .iter()
            .find(|(quest, _)| {
                !self.state.authoritative.quests.active.contains_key(quest)
                    && !self.state.authoritative.quests.completed.contains(quest)
            })
            .map(|(&quest, offer)| (quest, offer.giver));
        if let Some((quest, giver)) = offer {
            self.set_work(QuestWorkKey::AcquireQuest { quest: Some(quest) });
            tracing::info!(lane=?self.state.lane, quest, ?giver, "quest scheduler accepting authoritative quest offer");
            self.pending_accept = Some((quest, giver, Instant::now()));
            return self
                .propose_command(GameplayCommand::AcceptQuest { quest, giver }, true)
                .await;
        }

        let available_giver = self
            .state
            .authoritative
            .quests
            .giver_status
            .iter()
            .find(|(giver, status)| {
                quest_status_available(**status)
                    && self
                        .giver_retry_after
                        .get(giver)
                        .is_none_or(|(_, until)| *until <= Instant::now())
            })
            .map(|(&giver, &status)| (giver, status));
        if let Some((giver, status)) = available_giver {
            self.set_work(QuestWorkKey::AcquireQuest { quest: None });
            tracing::info!(lane=?self.state.lane, ?giver, status, "quest scheduler opening authoritative quest giver");
            self.pending_giver_interaction = Some((giver, Instant::now()));
            return self
                .propose_command(GameplayCommand::Interact(giver), true)
                .await;
        }

        if let Some(player) = self.active_mover_position() {
            let active_quests = &self.state.authoritative.quests.active;
            let radius = if active_quests.is_empty() {
                QUEST_START_EMPTY_LOG_RADIUS_YARDS
            } else {
                QUEST_START_HUB_SWEEP_RADIUS_YARDS
            };
            let class_id = self.state.authoritative.capabilities.class_id;
            if let Some(start) = wow_policy::questing::static_hints::nearby_quest_starts(
                player.map,
                player.point,
                class_id,
                radius,
            )
            .into_iter()
            .find(|start| {
                !self
                    .quest_start_search_attempts
                    .contains(&quest_start_search_point_key(player.map, start.location))
            }) {
                let work = self.set_work(QuestWorkKey::AcquireQuest { quest: None });
                tracing::info!(lane=?self.state.lane, work_id=?work.id, quest=start.quest_id, giver_entry=start.giver_entry_id, ?start.giver_kind, ?start.giver_name, x=start.location.x, y=start.location.y, distance=start.distance, radius, "quest scheduler searching static quest starter location");
                self.queue_movement(
                    start.location,
                    QUEST_START_ARRIVAL_RANGE,
                    Some(GameplayCommand::QueryQuestGivers),
                    None,
                    PlanOrigin::SystemPolicy,
                    work,
                    MovementPurpose::SearchArea,
                );
                return true;
            }
        }

        let incomplete_quest = self.next_incomplete_quest();
        if let Some(quest) = incomplete_quest {
            return self.tick_incomplete_quest(quest).await;
        }

        let complete_quest = self
            .state
            .authoritative
            .quests
            .active
            .iter()
            .find(|(_, progress)| progress.complete)
            .map(|(&quest, _)| quest);
        if let Some(quest) = complete_quest {
            return self.tick_complete_quest(quest).await;
        }

        self.set_work(QuestWorkKey::AcquireQuest { quest: None });
        tracing::debug!(lane=?self.state.lane, "quest scheduler requesting quest-giver statuses");
        self.propose_command(GameplayCommand::QueryQuestGivers, true)
            .await
    }

    async fn tick_incomplete_quest(&mut self, quest: u32) -> bool {
        if !self
            .state
            .authoritative
            .quests
            .definitions
            .contains_key(&quest)
        {
            self.set_work(QuestWorkKey::QueryDefinition { quest });
            tracing::info!(lane=?self.state.lane, quest, "quest scheduler requesting authoritative quest definition");
            return self
                .propose_command(GameplayCommand::QueryQuest { quest }, true)
                .await;
        }
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        let now = Instant::now();
        let mut excluded: BTreeSet<(usize, EntityId)> = self
            .credited_quest_targets
            .iter()
            .filter_map(|(credited_quest, objective, target)| {
                (*credited_quest == quest).then_some((*objective, *target))
            })
            .collect();
        excluded.extend(
            self.looted_corpses
                .iter()
                .map(|target| (usize::MAX, *target)),
        );
        excluded.extend(
            self.loot_retry_after
                .iter()
                .filter_map(|(target, (_, retry_after))| {
                    retry_after
                        .is_none_or(|deadline| now < deadline)
                        .then_some((usize::MAX, *target))
                }),
        );
        self.combat_target_retry_after
            .retain(|_, retry_after| *retry_after > now);
        excluded.extend(
            self.combat_target_retry_after
                .keys()
                .copied()
                .map(|target| (usize::MAX, target)),
        );
        match resolve_with_exclusions(&snapshot, quest, &excluded) {
            ObjectiveResolution::WaitingForDefinition => {
                self.waiting(format!(
                    "quest {quest} is waiting for authoritative definition"
                ));
                return true;
            }
            ObjectiveResolution::GroundedCreature { objective, target } => {
                self.set_work(QuestWorkKey::CombatObjective {
                    quest,
                    objective,
                    target,
                });
                tracing::info!(lane=?self.state.lane, quest, objective, ?target, "quest scheduler grounded creature objective from live object state");
                return self.dispatch_combat_target(target, false).await;
            }
            ObjectiveResolution::GroundedGameObject { objective, target } => {
                self.set_work(QuestWorkKey::InteractObjective {
                    quest,
                    objective,
                    target,
                });
                if !self.interaction_ready(quest, target) {
                    return true;
                }
                tracing::info!(lane=?self.state.lane, quest, objective, ?target, "quest scheduler grounded game-object objective from live object state");
                let baseline_progress = self.quest_objective_progress(quest, objective);
                return self
                    .dispatch_quest_semantic(
                        GameplayCommand::UseGameObject(target),
                        PendingQuestAction::QuestObjectUse {
                            quest,
                            target,
                            objective: Some(objective),
                            item: None,
                            baseline_progress,
                            baseline_count: 0,
                            started: Instant::now(),
                        },
                    )
                    .await;
            }
            ObjectiveResolution::GroundedScriptedItemUse {
                objective,
                target,
                item,
                spell,
                cast_count,
            } => {
                self.set_work(QuestWorkKey::InteractObjective {
                    quest,
                    objective,
                    target,
                });
                let Some(instance) = self
                    .state
                    .authoritative
                    .inventory
                    .usable_instance(item)
                    .cloned()
                else {
                    self.waiting(format!("quest {quest} needs usable item {item} for scripted objective, but no authoritative backpack slot/GUID is known"));
                    return true;
                };
                match wow_policy::questing::objectives::item_use_metadata_status(
                    &snapshot, item, spell,
                ) {
                    wow_policy::questing::objectives::ItemUseMetadataStatus::Missing => {
                        if self.queried_item_templates.insert(item) {
                            tracing::info!(lane=?self.state.lane, quest, item, spell, "requesting item metadata before quest item use");
                            return self
                                .propose_command(GameplayCommand::QueryItem { item }, false)
                                .await;
                        }
                        self.waiting(format!("quest {quest} is waiting for authoritative metadata for item {item} before use"));
                        return true;
                    }
                    wow_policy::questing::objectives::ItemUseMetadataStatus::Mismatch {
                        observed_spell,
                    } => {
                        self.waiting(format!("quest {quest} will not use item {item}: authoritative template spell {observed_spell} does not match grounded quest spell {spell}"));
                        return true;
                    }
                    wow_policy::questing::objectives::ItemUseMetadataStatus::Matches => {}
                }
                tracing::info!(lane=?self.state.lane, quest, objective, ?target, item, spell, slot=instance.backpack_slot, "quest scheduler using scripted targeted quest item");
                return self
                    .dispatch_quest_semantic(
                        GameplayCommand::UseItemInstance {
                            item,
                            item_guid: instance.guid,
                            backpack_slot: instance.backpack_slot,
                            spell,
                            target: Some(target),
                            cast_count,
                        },
                        self.pending_quest_credit(quest, objective, target, "scripted item use"),
                    )
                    .await;
            }
            ObjectiveResolution::GroundedQuestSpell {
                objective,
                target,
                spell,
                name,
            } => {
                self.set_work(QuestWorkKey::InteractObjective {
                    quest,
                    objective,
                    target,
                });
                let now_ms = Millis::wall_clock_now().0;
                if let Err(reason) = wow_policy::combat::readiness::check_spell_readiness(
                    &snapshot,
                    spell,
                    Some(target),
                    now_ms,
                ) {
                    self.waiting(format!(
                        "quest {quest} is waiting for {name} ({spell}): {reason:?}"
                    ));
                    return true;
                }
                tracing::info!(lane=?self.state.lane, quest, objective, ?target, spell, %name, "quest scheduler casting required spell on quest target");
                return self
                    .dispatch_quest_semantic(
                        GameplayCommand::Cast {
                            spell,
                            target: Some(target),
                        },
                        self.pending_quest_credit(quest, objective, target, name),
                    )
                    .await;
            }
            ObjectiveResolution::QuestSpellUnavailable {
                objective,
                target,
                name,
            } => {
                self.set_work(QuestWorkKey::InteractObjective {
                    quest,
                    objective,
                    target,
                });
                self.waiting(format!("quest {quest} needs {name} for target {target}, but the spell is not in the observed spellbook"));
                return true;
            }
            ObjectiveResolution::GroundedControlledSpell {
                objective,
                target,
                spell,
            } => {
                self.set_work(QuestWorkKey::InteractObjective {
                    quest,
                    objective,
                    target,
                });
                tracing::info!(lane=?self.state.lane, quest, objective, ?target, spell, mover=?self.state.authoritative.control.mover, "quest scheduler using observed controlled-unit ability for objective");
                return self
                    .dispatch_quest_semantic(
                        GameplayCommand::VehicleCast {
                            spell,
                            target: Some(target),
                        },
                        self.pending_quest_credit(
                            quest,
                            objective,
                            target,
                            "controlled quest ability",
                        ),
                    )
                    .await;
            }
            ObjectiveResolution::GroundedQuestTool {
                target,
                activation_spell,
            } => {
                self.set_work(QuestWorkKey::InteractObjective {
                    quest,
                    objective: usize::MAX,
                    target,
                });
                if !self.interaction_ready(quest, target) {
                    return true;
                }
                tracing::info!(lane=?self.state.lane, quest, ?target, ?activation_spell, "quest scheduler activating live quest-bound control object");
                let command = activation_spell
                    .map(|spell| GameplayCommand::CastGameObject {
                        spell,
                        target,
                        report_use: true,
                    })
                    .unwrap_or(GameplayCommand::UseGameObject(target));
                let pending = PendingQuestAction::ControlActivation {
                    target,
                    started: Instant::now(),
                };
                return self.dispatch_quest_semantic(command, pending).await;
            }
            ObjectiveResolution::QuestToolSearch { destination } => {
                let work = self.set_work(QuestWorkKey::TravelToObjective {
                    quest,
                    objective: usize::MAX,
                    destination,
                });
                if self.active_mover_position().is_some_and(|player| {
                    player.map == destination.map
                        && quest_tool_search_arrived(player.point, destination.point)
                }) {
                    self.waiting(format!("quest {quest} reached quest-control search area; waiting for live authoritative control object"));
                    return true;
                }
                tracing::info!(lane=?self.state.lane, quest, work_id=?work.id, map=destination.map, x=destination.point.x, y=destination.point.y, "quest scheduler traveling to quest-bound control object search area");
                self.queue_movement(
                    destination.point,
                    QUEST_TOOL_SEARCH_RANGE,
                    None,
                    None,
                    PlanOrigin::SystemPolicy,
                    work,
                    MovementPurpose::SearchArea,
                );
                return true;
            }
            ObjectiveResolution::GroundedItemCreature { item, target, dead } => {
                self.set_work(QuestWorkKey::CollectItem { quest, item });
                if dead {
                    tracing::info!(lane=?self.state.lane, quest, item, ?target, "quest scheduler looting grounded quest-item source");
                    let baseline_count = self.quest_item_count(item);
                    return self
                        .dispatch_quest_semantic(
                            GameplayCommand::Loot(target),
                            PendingQuestAction::Loot {
                                item,
                                target,
                                baseline_count,
                                baseline_generation: self
                                    .state
                                    .authoritative
                                    .inventory
                                    .bot_loot_generation,
                                started: Instant::now(),
                            },
                        )
                        .await;
                }
                tracing::info!(lane=?self.state.lane, quest, item, ?target, "quest scheduler engaging grounded quest-item source through shared combat selector");
                return self.dispatch_combat_target(target, false).await;
            }
            ObjectiveResolution::GroundedItemGameObject { item, target } => {
                self.set_work(QuestWorkKey::CollectItem { quest, item });
                if !self.interaction_ready(quest, target) {
                    return true;
                }
                tracing::info!(lane=?self.state.lane, quest, item, ?target, "quest scheduler using grounded game-object quest-item source");
                let baseline_count = self.quest_item_count(item);
                return self
                    .dispatch_quest_semantic(
                        quest_item_gameobject_open_command(target),
                        PendingQuestAction::QuestObjectUse {
                            quest,
                            target,
                            objective: None,
                            item: Some(item),
                            baseline_progress: 0,
                            baseline_count,
                            started: Instant::now(),
                        },
                    )
                    .await;
            }
            ObjectiveResolution::SearchArea {
                objective,
                mut destination,
                mut alternatives,
                source,
            } => {
                let cross_map = self
                    .state
                    .authoritative
                    .position
                    .player
                    .is_some_and(|player| player.map != destination.map);
                if source == "server-poi" || cross_map {
                    let Some(controller) = self.movement_controller.as_ref() else {
                        self.waiting(format!("quest {quest} travel goal is on map {}; navigation controller is unavailable", destination.map));
                        return true;
                    };
                    match controller.project_grounded_waypoint(destination) {
                        Ok(projected) => {
                            destination = projected;
                            if source == "server-poi" {
                                alternatives = vec![projected];
                            }
                        }
                        Err(error) => {
                            self.waiting(format!("quest {quest} destination is not a validated ground point: {error:?}"));
                            return true;
                        }
                    }
                }
                let work = self.set_work(QuestWorkKey::TravelToObjective {
                    quest,
                    objective,
                    destination,
                });
                if cross_map {
                    self.queue_world_movement(
                        destination,
                        QUEST_SEARCH_ARRIVAL_RANGE,
                        None,
                        None,
                        PlanOrigin::SystemPolicy,
                        work,
                        MovementPurpose::SearchArea,
                    );
                    return true;
                }
                let key = (quest, objective, 0);
                let current_pos = self.active_mover_position().map(|position| position.point);
                let arrived = current_pos
                    .is_some_and(|position| quest_search_arrived(position, destination.point));
                let candidates = bounded_candidates(
                    alternatives
                        .into_iter()
                        .map(|position| position.point)
                        .collect(),
                    current_pos.unwrap_or(destination.point),
                );
                let Some(destination) = self.next_search_destination(
                    key,
                    &candidates,
                    arrived.then_some(destination.point),
                ) else {
                    let origin = current_pos.unwrap_or(destination.point);
                    let roam = self.next_search_roam_destination(key, origin);
                    tracing::info!(lane=?self.state.lane, quest, objective, x=roam.x, y=roam.y, "quest objective search found no new hint; roaming before another search");
                    self.queue_search_movement(roam, work).await;
                    return true;
                };
                tracing::info!(lane=?self.state.lane, quest, objective, work_id=?work.id, %source, x=destination.x, y=destination.y, "quest scheduler starting bounded objective-area travel");
                self.queue_search_movement(destination, work).await;
                return true;
            }
            ObjectiveResolution::ItemCollection {
                item,
                required,
                current,
                destinations,
            } => {
                let work = self.set_work(QuestWorkKey::CollectItem { quest, item });
                let key = (quest, usize::MAX, item);
                let active_position = self.active_mover_position();
                let current_pos = active_position.map(|position| position.point);
                let candidates = bounded_candidates_within(
                    destinations
                        .into_iter()
                        .filter(|position| {
                            active_position.is_some_and(|active| active.map == position.map)
                        })
                        .map(|position| position.point)
                        .collect(),
                    current_pos.unwrap_or_default(),
                    QUEST_ITEM_SOURCE_SEARCH_RADIUS,
                );
                let reached_destination = current_pos.and_then(|position| {
                    candidates
                        .iter()
                        .copied()
                        .filter(|point| quest_search_arrived(position, *point))
                        .min_by(|a, b| position.distance(*a).total_cmp(&position.distance(*b)))
                });
                let Some(destination) =
                    self.next_search_destination(key, &candidates, reached_destination)
                else {
                    let origin = current_pos.unwrap_or_default();
                    let roam = self.next_search_roam_destination(key, origin);
                    tracing::info!(lane=?self.state.lane, quest, item, current, required, x=roam.x, y=roam.y, "quest item search found no new source hint; roaming before another search");
                    self.queue_search_movement(roam, work).await;
                    return true;
                };
                if reached_destination != Some(destination) {
                    tracing::info!(lane=?self.state.lane, quest, item, current, required, work_id=?work.id, x=destination.x, y=destination.y, "quest item objective using AzerothCore loot-source search hint");
                    self.queue_search_movement(destination, work).await;
                }
                return true;
            }
            ObjectiveResolution::NoSupportedObjective => {
                self.waiting(format!("quest {quest} has no remaining supported objective in the authoritative definition"));
                return true;
            }
        }
    }

    async fn tick_complete_quest(&mut self, quest: u32) -> bool {
        self.set_work(QuestWorkKey::TurnIn { quest });
        if let Some(dialog) = self.state.authoritative.quests.turn_in.get(&quest).cloned() {
            match dialog.stage {
                wow_state::quests::QuestTurnInStage::RequestItems { can_complete: true } => {
                    tracing::info!(lane=?self.state.lane, quest, giver=?dialog.giver, "quest turn-in requesting reward after required-items confirmation");
                    self.pending_turn_in = Some((quest, Instant::now(), "request-reward"));
                    return self
                        .propose_command(
                            GameplayCommand::RequestQuestReward {
                                quest,
                                giver: dialog.giver,
                            },
                            true,
                        )
                        .await;
                }
                wow_state::quests::QuestTurnInStage::RequestItems {
                    can_complete: false,
                } => {
                    self.waiting(format!("quest {quest} turn-in dialog reports required items or money are still incomplete"));
                    return true;
                }
                wow_state::quests::QuestTurnInStage::OfferReward { reward_items } => {
                    let snapshot = Snapshot::from_state(&self.state.authoritative);
                    if self
                        .reward_metadata_waiting
                        .is_none_or(|(pending_quest, _)| pending_quest != quest)
                    {
                        self.reward_metadata_waiting = Some((quest, Instant::now()));
                    }
                    let missing = wow_policy::questing::rewards::missing_score_metadata(
                        &snapshot,
                        &reward_items,
                    );
                    let started = self.reward_metadata_waiting.expect("initialized above").1;
                    let within_wait =
                        started.elapsed() < wow_policy::questing::rewards::metadata_wait();
                    if within_wait {
                        if let Some(item) = missing
                            .iter()
                            .copied()
                            .find(|item| self.queried_item_templates.insert(*item))
                        {
                            tracing::debug!(lane=?self.state.lane, quest, item, "requesting item metadata to compare quest rewards");
                            return self
                                .propose_command(GameplayCommand::QueryItem { item }, false)
                                .await;
                        }
                        if !missing.is_empty() {
                            self.waiting(format!(
                                "quest {quest} is waiting for item metadata to compare reward choices"
                            ));
                            return true;
                        }
                    }
                    if !missing.is_empty() {
                        tracing::warn!(lane=?self.state.lane, quest, missing_items=?missing, "quest reward metadata wait expired; selecting from available scores");
                    }
                    let reward =
                        wow_policy::questing::rewards::best_reward_index(&snapshot, &reward_items);
                    tracing::info!(lane=?self.state.lane, quest, giver=?dialog.giver, reward, reward_items=?reward_items, "quest turn-in selected a reward from authoritative offer data");
                    self.reward_metadata_waiting = None;
                    self.pending_turn_in = Some((quest, Instant::now(), "choose-reward"));
                    return self
                        .propose_command(
                            GameplayCommand::ChooseQuestReward {
                                quest,
                                giver: dialog.giver,
                                reward,
                            },
                            true,
                        )
                        .await;
                }
            }
        }
        let reward_giver = self
            .state
            .authoritative
            .quests
            .giver_status
            .iter()
            .find(|(_, status)| quest_status_reward(**status))
            .map(|(&giver, &status)| (giver, status));
        if let Some((giver, status)) = reward_giver {
            tracing::info!(lane=?self.state.lane, quest, ?giver, status, "quest scheduler opening authoritative turn-in giver");
            self.pending_turn_in = Some((quest, Instant::now(), "complete-quest"));
            return self
                .propose_command(GameplayCommand::TurnInQuest { quest, giver }, true)
                .await;
        }
        if let Some(player) = self.active_mover_position() {
            if self.turn_in_search_hint_blocked(quest, player) {
                self.waiting(format!(
                    "quest {quest} turn-in search hint has no route from this area; waiting for a live reward giver or a new position"
                ));
                if self
                    .last_turn_in_search_query
                    .is_none_or(|(queried_quest, at)| {
                        queried_quest != quest || at.elapsed() >= Duration::from_secs(10)
                    })
                {
                    self.last_turn_in_search_query = Some((quest, Instant::now()));
                    return self
                        .propose_command(GameplayCommand::QueryQuestGivers, true)
                        .await;
                }
                return true;
            }
            if let Some(destination) =
                wow_policy::questing::static_hints::nearest_turn_in(quest, player.map, player.point)
            {
                if turn_in_search_arrived(player.point, destination) {
                    self.waiting(format!("quest {quest} reached turn-in search area; waiting for live authoritative reward giver"));
                    if self
                        .last_turn_in_search_query
                        .is_none_or(|(queried_quest, at)| {
                            queried_quest != quest || at.elapsed() >= Duration::from_secs(10)
                        })
                    {
                        self.last_turn_in_search_query = Some((quest, Instant::now()));
                        return self
                            .propose_command(GameplayCommand::QueryQuestGivers, true)
                            .await;
                    }
                    return true;
                }
                let work = self.current_work.clone().expect("turn-in work exists");
                tracing::info!(lane=?self.state.lane, quest, work_id=?work.id, x=destination.x, y=destination.y, "quest scheduler using AzerothCore turn-in search hint");
                self.queue_movement(
                    destination,
                    TURN_IN_SEARCH_RANGE,
                    Some(GameplayCommand::QueryQuestGivers),
                    None,
                    PlanOrigin::SystemPolicy,
                    work,
                    MovementPurpose::SearchArea,
                );
                return true;
            }
        }
        tracing::debug!(lane=?self.state.lane, quest, "completed quest has no observed reward giver; refreshing quest-giver statuses");
        return self
            .propose_command(GameplayCommand::QueryQuestGivers, true)
            .await;
    }

    fn set_work(&mut self, key: QuestWorkKey) -> QuestWorkRuntime {
        if let Some(work) = &self.current_work {
            if work.key == key {
                return work.clone();
            }
        }
        let work = QuestWorkRuntime {
            id: QuestWorkId(self.next_work),
            key,
        };
        self.next_work = self.next_work.wrapping_add(1).max(1);
        tracing::info!(lane=?self.state.lane, work_id=?work.id, key=?work.key, "quest semantic work selected");
        self.current_work = Some(work.clone());
        work
    }

    fn clear_quest_search_focus(&mut self, failed_work_id: QuestWorkId) {
        if self
            .current_work
            .as_ref()
            .is_some_and(|work| work.id == failed_work_id)
        {
            self.current_work = None;
        }
    }

    fn turn_in_search_hint_blocked(&mut self, quest: u32, player: WorldPosition) -> bool {
        let Some((failed_map, failed_from)) = self.turn_in_search_failures.get(&quest).copied()
        else {
            return false;
        };
        if failed_map == player.map
            && failed_from.distance(player.point) < TURN_IN_SEARCH_RETRY_DISTANCE
        {
            return true;
        }
        self.turn_in_search_failures.remove(&quest);
        false
    }

    fn record_search_arrival(&mut self, movement: &PendingMovement) {
        if movement.purpose != MovementPurpose::SearchArea {
            return;
        }
        if matches!(
            &movement.work.key,
            QuestWorkKey::AcquireQuest { quest: None }
        ) {
            if let Some(player) = self.active_mover_position() {
                self.quest_start_search_attempts
                    .insert(quest_start_search_point_key(
                        player.map,
                        movement.destination.point,
                    ));
            }
            return;
        }
        let key = match &movement.work.key {
            QuestWorkKey::TravelToObjective {
                quest, objective, ..
            } => (*quest, *objective, 0),
            QuestWorkKey::CollectItem { quest, item } => (*quest, usize::MAX, *item),
            _ => return,
        };
        self.search_attempts
            .entry(key)
            .or_default()
            .insert(search_point_key(movement.destination.point));
    }

    fn record_failed_search_destination(&mut self, movement: &PendingMovement) {
        if let QuestWorkKey::TurnIn { quest } = &movement.work.key {
            if let Some(position) = self.active_mover_position() {
                self.record_turn_in_search_failure(*quest, position);
            }
            return;
        }
        if matches!(
            &movement.work.key,
            QuestWorkKey::AcquireQuest { quest: None }
        ) {
            self.quest_start_search_attempts
                .insert(quest_start_search_point_key(
                    movement.destination.map,
                    movement.destination.point,
                ));
            return;
        }
        let key = match &movement.work.key {
            QuestWorkKey::TravelToObjective {
                quest, objective, ..
            } => (*quest, *objective, 0),
            QuestWorkKey::CollectItem { quest, item } => (*quest, usize::MAX, *item),
            _ => return,
        };
        self.search_attempts
            .entry(key)
            .or_default()
            .insert(search_point_key(movement.destination.point));
    }

    fn record_turn_in_search_failure(&mut self, quest: u32, player: WorldPosition) {
        self.turn_in_search_failures
            .insert(quest, (player.map, player.point));
    }

    fn defer_quest_giver(&mut self, giver: EntityId) {
        let attempts = self
            .giver_retry_after
            .get(&giver)
            .map_or(1, |(attempts, _)| attempts.saturating_add(1));
        self.giver_retry_after.insert(
            giver,
            (attempts, Instant::now() + giver_retry_delay(attempts)),
        );
    }

    fn quest_objective_progress(&self, quest: u32, objective: usize) -> u32 {
        self.state
            .authoritative
            .quests
            .active
            .get(&quest)
            .and_then(|progress| progress.objectives.get(objective))
            .copied()
            .unwrap_or_default()
    }

    fn quest_item_count(&self, item: u32) -> u32 {
        self.state.authoritative.inventory.count(item)
    }

    fn pending_quest_credit(
        &self,
        quest: u32,
        objective: usize,
        target: EntityId,
        label: &'static str,
    ) -> PendingQuestAction {
        PendingQuestAction::QuestCredit {
            quest,
            objective,
            baseline: self.quest_objective_progress(quest, objective),
            target,
            started: Instant::now(),
            label,
        }
    }

    fn defer_quest_interaction(&mut self, quest: u32, target: EntityId) -> (u8, Duration) {
        if !self.interaction_retry_after.contains_key(&(quest, target))
            && self.interaction_retry_after.len() >= MAX_INTERACTION_RETRIES
        {
            if let Some(oldest) = self
                .interaction_retry_after
                .iter()
                .min_by_key(|(_, (_, until))| *until)
                .map(|(key, _)| *key)
            {
                self.interaction_retry_after.remove(&oldest);
            }
        }
        let attempts = self
            .interaction_retry_after
            .get(&(quest, target))
            .map_or(1, |(attempts, _)| attempts.saturating_add(1));
        let delay = giver_retry_delay(attempts);
        self.interaction_retry_after
            .insert((quest, target), (attempts, Instant::now() + delay));
        (attempts, delay)
    }

    fn interaction_ready(&mut self, quest: u32, target: EntityId) -> bool {
        match self.interaction_retry_after.get(&(quest, target)).copied() {
            Some((_, until)) if until > Instant::now() => {
                self.waiting(format!("quest {quest} object {target} has no authoritative progress yet; retry allowed after {} seconds", until.duration_since(Instant::now()).as_secs()));
                false
            }
            _ => true,
        }
    }

    fn next_search_destination(
        &mut self,
        key: (u32, usize, u32),
        candidates: &[Vec3],
        reached: Option<Vec3>,
    ) -> Option<Vec3> {
        let now = Instant::now();
        if self
            .search_retry_after
            .get(&key)
            .is_some_and(|deadline| *deadline > now)
        {
            return None;
        }
        self.search_retry_after.remove(&key);
        let tried = self.search_attempts.entry(key).or_default();
        if let Some(point) = reached {
            tried.insert(search_point_key(point));
        }
        if let Some(point) = candidates
            .iter()
            .take(5)
            .copied()
            .find(|point| !tried.contains(&search_point_key(*point)))
        {
            return Some(point);
        }
        tried.clear();
        self.search_retry_after
            .insert(key, now + Duration::from_secs(5));
        None
    }

    fn next_search_roam_destination(&mut self, key: (u32, usize, u32), origin: Vec3) -> Vec3 {
        let cursor = self.search_roam_cursor.entry(key).or_default();
        let angle = f32::from(*cursor % 8) * std::f32::consts::FRAC_PI_4;
        *cursor = cursor.wrapping_add(1);
        Vec3::new(
            origin.x + angle.cos() * QUEST_SEARCH_ROAM_RADIUS,
            origin.y + angle.sin() * QUEST_SEARCH_ROAM_RADIUS,
            origin.z,
        )
    }

    fn pending_quest_action_blocks(&mut self) -> bool {
        let Some(pending) = self.pending_quest_action.clone() else {
            return false;
        };
        match pending {
            PendingQuestAction::CorpseLootDelay { target, arrived_at } => {
                if self.state.authoritative.inventory.current_loot == Some(target)
                    && matches!(
                        self.state.authoritative.inventory.current_loot_owner,
                        Some(wow_state::LootOwnership::Player)
                    )
                {
                    self.pending_quest_action = None;
                    self.post_combat_loot = None;
                    return false;
                }
                if arrived_at.elapsed() < Duration::from_secs(1) {
                    self.waiting(format!(
                        "reached corpse {target}; waiting one second before looting"
                    ));
                    return true;
                }
                self.pending_quest_action = None;
                self.corpses_ready_to_loot.insert(target);
                false
            }
            PendingQuestAction::Combat {
                target,
                target_state,
                started,
                cycle,
            } => {
                let dead_or_gone = self
                    .state
                    .authoritative
                    .entities
                    .0
                    .get(&target)
                    .is_none_or(wow_state::entities::EntityState::is_dead);
                if dead_or_gone {
                    let corpse_present = self
                        .state
                        .authoritative
                        .entities
                        .0
                        .get(&target)
                        .is_some_and(wow_state::entities::EntityState::is_dead);
                    tracing::info!(lane=?self.state.lane, ?target, corpse_present, "authoritative combat completion observed");
                    let mut corpse = self
                        .state
                        .authoritative
                        .entities
                        .0
                        .get(&target)
                        .cloned()
                        .or(target_state);
                    if let Some(corpse) = &mut corpse {
                        corpse.mark_dead();
                        if relocate_moved_mob_corpse(&self.state.authoritative, corpse) {
                            if let Some(entity) =
                                self.state.authoritative.entities.0.get_mut(&target)
                            {
                                entity.position = corpse.position;
                            }
                            tracing::info!(lane=?self.state.lane, ?target, position=?corpse.position, "using the nearby killer position for a moved mob corpse");
                        }
                    }
                    self.post_combat_loot = Some((target, Instant::now(), corpse));
                    self.pending_quest_action = None;
                    return false;
                }
                if started.elapsed() < cycle {
                    self.waiting(format!("combat action against {target} is in progress; waiting for authoritative death/despawn or next combat cycle"));
                    return true;
                }
                tracing::debug!(lane=?self.state.lane, ?target, cycle_ms=cycle.as_millis(), "combat cycle elapsed; re-running shared combat selector");
                self.pending_quest_action = None;
                false
            }
            PendingQuestAction::CorpseLoot {
                target,
                baseline_generation,
                started,
            } => {
                let generation = self.state.authoritative.inventory.bot_loot_generation;
                if self.state.authoritative.inventory.current_loot == Some(target)
                    && matches!(
                        self.state.authoritative.inventory.current_loot_owner,
                        Some(wow_state::LootOwnership::Player)
                    )
                {
                    tracing::info!(lane=?self.state.lane, ?target, "pending bot corpse loot was superseded by player loot; cancelling without bot-success credit");
                    self.pending_quest_action = None;
                    self.post_combat_loot = None;
                    return false;
                }
                if generation.saturating_sub(baseline_generation) >= 2
                    && self.state.authoritative.inventory.current_loot.is_none()
                {
                    tracing::info!(lane=?self.state.lane, ?target, baseline_generation, generation, "authoritative bot-owned post-combat loot transaction completed");
                    self.looted_corpses.insert(target);
                    self.pending_quest_action = None;
                    self.post_combat_loot = None;
                    return false;
                }
                if !self.state.authoritative.entities.0.contains_key(&target)
                    && generation > baseline_generation
                {
                    self.pending_quest_action = None;
                    self.post_combat_loot = None;
                    return false;
                }
                if started.elapsed() < Duration::from_secs(6) {
                    self.waiting(format!(
                        "post-combat loot on {target} is awaiting authoritative loot transaction"
                    ));
                    return true;
                }
                tracing::warn!(lane=?self.state.lane, ?target, baseline_generation, generation, "post-combat loot timed out; allowing quest scheduler to continue");
                self.pending_quest_action = None;
                self.post_combat_loot = None;
                false
            }
            PendingQuestAction::Loot {
                item,
                target,
                baseline_count,
                baseline_generation,
                started,
            } => {
                let current = self.quest_item_count(item);
                let generation = self.state.authoritative.inventory.bot_loot_generation;
                let player_superseded = self.state.authoritative.inventory.current_loot
                    == Some(target)
                    && matches!(
                        self.state.authoritative.inventory.current_loot_owner,
                        Some(wow_state::LootOwnership::Player)
                    );
                let gone = !self.state.authoritative.entities.0.contains_key(&target);
                if player_superseded {
                    tracing::info!(lane=?self.state.lane, item, ?target, "pending bot loot was superseded by player loot; cancelling without bot-success credit");
                    self.pending_quest_action = None;
                    return false;
                }
                let transaction_completed = generation.saturating_sub(baseline_generation) >= 2
                    && self.state.authoritative.inventory.current_loot.is_none();
                if current > baseline_count {
                    tracing::info!(lane=?self.state.lane, item, ?target, baseline_count, current, baseline_generation, generation, "authoritative quest item count increased after looting");
                    self.looted_corpses.insert(target);
                    self.pending_quest_action = None;
                    return false;
                }
                if transaction_completed {
                    tracing::info!(lane=?self.state.lane, item, ?target, baseline_count, current, baseline_generation, generation, "loot transaction closed without increasing the required quest item count");
                    self.looted_corpses.insert(target);
                    self.pending_quest_action = None;
                    return false;
                }
                if gone {
                    tracing::warn!(lane=?self.state.lane, item, ?target, baseline_count, current, "corpse disappeared before the required quest item count increased");
                    self.pending_quest_action = None;
                    return false;
                }
                if started.elapsed() < Duration::from_secs(6) {
                    self.waiting(format!("loot action for item {item} on {target} is in progress; waiting for inventory/despawn evidence"));
                    return true;
                }
                tracing::warn!(lane=?self.state.lane, item, ?target, "loot action timed out without authoritative inventory progress; allowing bounded retry");
                self.pending_quest_action = None;
                false
            }
            PendingQuestAction::Interact { target, started } => {
                if !self.state.authoritative.entities.0.contains_key(&target) {
                    self.pending_quest_action = None;
                    return false;
                }
                if started.elapsed() < Duration::from_secs(3) {
                    self.waiting(format!(
                        "interaction with {target} is awaiting authoritative state change"
                    ));
                    return true;
                }
                self.pending_quest_action = None;
                false
            }
            PendingQuestAction::QuestObjectUse {
                quest,
                target,
                objective,
                item,
                baseline_progress,
                baseline_count,
                started,
            } => {
                let progress = objective
                    .and_then(|slot| {
                        self.state
                            .authoritative
                            .quests
                            .active
                            .get(&quest)
                            .and_then(|state| state.objectives.get(slot))
                            .copied()
                    })
                    .unwrap_or(baseline_progress);
                let count = item
                    .map(|id| self.state.authoritative.inventory.count(id))
                    .unwrap_or(baseline_count);
                let gone = !self.state.authoritative.entities.0.contains_key(&target);
                if progress > baseline_progress || count > baseline_count || gone {
                    self.interaction_retry_after.remove(&(quest, target));
                    tracing::info!(lane=?self.state.lane, quest, ?objective, ?item, ?target, baseline_progress, progress, baseline_count, count, gone, "authoritative quest object-use progress observed");
                    self.pending_quest_action = None;
                    return false;
                }
                if started.elapsed() < Duration::from_secs(4) {
                    self.waiting(format!("quest {quest} object use on {target} is awaiting objective, inventory, or despawn evidence"));
                    return true;
                }
                let (attempts, delay) = self.defer_quest_interaction(quest, target);
                tracing::warn!(lane=?self.state.lane, quest, ?objective, ?item, ?target, attempts, retry_after_ms=delay.as_millis(), baseline_progress, progress, baseline_count, count, "quest object use produced no authoritative progress; applying bounded retry delay");
                self.pending_quest_action = None;
                false
            }
            PendingQuestAction::ControlActivation { target, started } => {
                let quest = self
                    .current_work
                    .as_ref()
                    .and_then(|work| match work.key {
                        QuestWorkKey::InteractObjective { quest, .. } => Some(quest),
                        _ => None,
                    })
                    .unwrap_or_default();
                let item_target_ready = quest != 0
                    && matches!(
                        resolve_objective(&Snapshot::from_state(&self.state.authoritative), quest),
                        ObjectiveResolution::GroundedScriptedItemUse { .. }
                    );
                if self.state.authoritative.control.mover.is_some() || item_target_ready {
                    tracing::info!(lane=?self.state.lane, quest, ?target, mover=?self.state.authoritative.control.mover, item_target_ready, "authoritative quest state advanced after quest-tool activation");
                    self.pending_quest_action = None;
                    return false;
                }
                if started.elapsed() < Duration::from_secs(12) {
                    self.waiting(format!("quest control activation on {target} is waiting for authoritative controlled mover"));
                    return true;
                }
                let (attempts, delay) = self.defer_quest_interaction(quest, target);
                tracing::warn!(lane=?self.state.lane, quest, ?target, attempts, retry_after_ms=delay.as_millis(), "quest control activation timed out without authoritative mover; applying bounded retry delay");
                self.pending_quest_action = None;
                false
            }
            PendingQuestAction::QuestCredit {
                quest,
                objective,
                baseline,
                target,
                started,
                label,
            } => {
                let current = self
                    .state
                    .authoritative
                    .quests
                    .active
                    .get(&quest)
                    .and_then(|progress| progress.objectives.get(objective))
                    .copied()
                    .unwrap_or(baseline);
                if current > baseline {
                    tracing::info!(lane=?self.state.lane, quest, objective, baseline, current, ?target, %label, "authoritative quest credit observed");
                    self.credited_quest_targets
                        .insert((quest, objective, target));
                    self.pending_quest_action = None;
                    return false;
                }
                if started.elapsed() < Duration::from_secs(12) {
                    self.waiting(format!("{label} on {target} is awaiting authoritative quest credit for quest {quest} objective {objective}"));
                    return true;
                }
                tracing::warn!(lane=?self.state.lane, quest, objective, baseline, current, ?target, %label, "quest-credit action timed out; allowing bounded retry");
                self.pending_quest_action = None;
                false
            }
        }
    }

    fn cancel_player_superseded_loot(&mut self, target: EntityId) {
        if matches!(
            self.pending_quest_action,
            Some(PendingQuestAction::Loot { target: pending, .. }
                | PendingQuestAction::CorpseLoot { target: pending, .. }
                | PendingQuestAction::CorpseLootDelay { target: pending, .. })
                if pending == target
        ) {
            self.pending_quest_action = None;
        }
        if self
            .post_combat_loot
            .as_ref()
            .is_some_and(|(pending, _, _)| *pending == target)
        {
            self.post_combat_loot = None;
        }
    }

    fn snapshot_with_post_combat_corpse(
        &self,
        mut snapshot: Snapshot,
        target: EntityId,
    ) -> Snapshot {
        if let Some((queued_target, _, Some(cached_target))) = &self.post_combat_loot
            && *queued_target == target
        {
            // The cached corpse can contain a kill-site correction. It must
            // replace a stale spawn position that is still in authoritative state.
            snapshot
                .state
                .entities
                .0
                .insert(target, cached_target.clone());
        }
        snapshot
    }

    async fn dispatch_combat_target(&mut self, target: EntityId, recovery: bool) -> bool {
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        if let Some(command) = self.select_combat_healthstone(&snapshot, target, recovery) {
            tracing::info!(lane=?self.state.lane, ?target, ?command, "emergency healthstone selected");
            let proposed = self.propose_combat_item(command, recovery).await;
            if proposed {
                self.combat_healthstone_retry_until =
                    Some(Instant::now() + COMBAT_HEALTHSTONE_RETRY);
            }
            return proposed;
        }
        if let Some(command) = self.select_combat_potion(&snapshot, target, recovery) {
            tracing::info!(lane=?self.state.lane, ?target, ?command, "critical combat potion selected");
            let proposed = self.propose_combat_item(command, recovery).await;
            if proposed {
                self.combat_potion_lockout_until =
                    Some(Instant::now() + COMBAT_POTION_FALLBACK_LOCKOUT);
            }
            return proposed;
        }
        if let Some(command) = self.select_combat_emergency_health_item(&snapshot, target, recovery)
        {
            tracing::info!(lane=?self.state.lane, ?target, ?command, "emergency health item selected");
            let proposed = self.propose_combat_item(command, recovery).await;
            if proposed {
                self.combat_emergency_health_item_retry_until =
                    Some(Instant::now() + COMBAT_EMERGENCY_HEALTH_ITEM_RETRY);
            }
            return proposed;
        }
        let selected = match wow_policy::combat::selector::select_action(&snapshot, target) {
            Ok(selected) => selected,
            Err(reason) => {
                let source = if recovery { "survival" } else { "quest" };
                let details = (reason == "no_safe_offensive_fallback")
                    .then(|| {
                        format!(
                            "; combat diagnostics: {}",
                            wow_policy::combat::selector::deferred_diagnostics(&snapshot, target)
                        )
                    })
                    .unwrap_or_default();
                self.waiting(format!(
                    "{source} combat against {target} deferred: {reason}{details}"
                ));
                return true;
            }
        };
        tracing::info!(lane=?self.state.lane, ?target, recovery, command=?selected.command, "shared combat selector chose action");
        if recovery {
            self.last_survival_action = Some((target, Instant::now()));
            self.propose_recovery(selected.command).await
        } else {
            self.dispatch_quest_semantic(
                selected.command,
                self.combat_pending(target, selected.cycle),
            )
            .await
        }
    }

    async fn propose_combat_item(&mut self, command: GameplayCommand, recovery: bool) -> bool {
        if recovery {
            self.propose_recovery(command).await
        } else {
            self.propose_command(command, false).await
        }
    }

    fn select_combat_healthstone(
        &self,
        snapshot: &Snapshot,
        target: EntityId,
        recovery: bool,
    ) -> Option<GameplayCommand> {
        if !combat_item_use_authorized(snapshot, target, recovery)
            || !combat_item_retry_ready(self.combat_healthstone_retry_until, Instant::now())
        {
            return None;
        }
        let selection =
            wow_policy::combat::consumables::select_emergency_healthstone(snapshot, target)?;
        Some(combat_item_command(
            selection,
            EntityId(snapshot.state.session.character_guid?),
        ))
    }

    fn select_combat_emergency_health_item(
        &self,
        snapshot: &Snapshot,
        target: EntityId,
        recovery: bool,
    ) -> Option<GameplayCommand> {
        if !combat_item_use_authorized(snapshot, target, recovery)
            || !combat_item_retry_ready(
                self.combat_emergency_health_item_retry_until,
                Instant::now(),
            )
        {
            return None;
        }
        let self_heal_available = wow_policy::combat::selector::has_legal_self_heal(snapshot);
        let selection = wow_policy::combat::consumables::select_emergency_health_item(
            snapshot,
            target,
            self_heal_available,
        )?;
        Some(combat_item_command(
            selection,
            EntityId(snapshot.state.session.character_guid?),
        ))
    }

    fn select_combat_potion(
        &self,
        snapshot: &Snapshot,
        target: EntityId,
        recovery: bool,
    ) -> Option<GameplayCommand> {
        let player = EntityId(snapshot.state.session.character_guid?);
        let confirmed_in_combat = recovery
            || snapshot
                .state
                .entities
                .0
                .get(&player)
                .and_then(wow_state::entities::EntityState::in_combat)
                == Some(true)
            || snapshot
                .state
                .entities
                .0
                .get(&target)
                .and_then(wow_state::entities::EntityState::in_combat)
                == Some(true);
        if !confirmed_in_combat {
            return None;
        }
        let health_percent = snapshot
            .state
            .entities
            .0
            .get(&player)
            .and_then(|entity| entity.health)
            .filter(|(_, maximum)| *maximum > 0)
            .map(|(health, maximum)| health.saturating_mul(100) / maximum);
        let self_heal_available = wow_policy::combat::selector::has_legal_self_heal(snapshot);
        let healer = matches!(
            &self.state.mission.intent,
            wow_domain::MissionIntent::Party {
                role: wow_domain::GroupRole::Healer
            } | wow_domain::MissionIntent::Raid {
                role: wow_domain::GroupRole::Healer
            }
        ) || matches!(
            (
                snapshot.state.capabilities.class_id,
                snapshot.state.capabilities.specialization_tree,
            ),
            (Some(2), Some(0)) | (Some(5), Some(0 | 1)) | (Some(7), Some(2)) | (Some(11), Some(2))
        );
        let target_health = snapshot
            .state
            .entities
            .0
            .get(&target)
            .and_then(|entity| entity.health)
            .filter(|(_, maximum)| *maximum > 0)
            .map(|(health, maximum)| health.saturating_mul(100) / maximum);
        let active_mana_work = healer || target_health.is_some_and(|health| health > 20);
        let now = Instant::now();
        let shared_lockout_ready =
            combat_potion_lockout_ready(self.combat_potion_lockout_until, now);
        let selection = wow_policy::combat::consumables::select_combat_potion(
            snapshot,
            target,
            health_percent.map(|_| (25, self_heal_available)),
            Some((if healer { 20 } else { 15 }, active_mana_work)),
            shared_lockout_ready,
        )?;
        Some(combat_item_command(selection, player))
    }

    fn combat_pending(&self, target: EntityId, cycle: Duration) -> PendingQuestAction {
        PendingQuestAction::Combat {
            target,
            target_state: self.state.authoritative.entities.0.get(&target).cloned(),
            started: Instant::now(),
            cycle,
        }
    }

    async fn dispatch_resumed_quest_action(
        &mut self,
        command: GameplayCommand,
        work: &QuestWorkKey,
    ) -> bool {
        let pending = match command.clone() {
            GameplayCommand::Attack(target) => {
                Some(self.combat_pending(target, Duration::from_secs(12)))
            }
            GameplayCommand::Cast {
                target: Some(target),
                ..
            } if matches!(
                work,
                QuestWorkKey::CombatObjective { .. } | QuestWorkKey::CollectItem { .. }
            ) =>
            {
                Some(self.combat_pending(target, Duration::from_secs(3)))
            }
            GameplayCommand::Loot(target) => {
                let item = match work {
                    QuestWorkKey::CollectItem { item, .. } => *item,
                    _ => 0,
                };
                let baseline_count = self.quest_item_count(item);
                Some(PendingQuestAction::Loot {
                    item,
                    target,
                    baseline_count,
                    baseline_generation: self.state.authoritative.inventory.bot_loot_generation,
                    started: Instant::now(),
                })
            }
            GameplayCommand::UseGameObject(target) | GameplayCommand::Interact(target) => {
                Some(PendingQuestAction::Interact {
                    target,
                    started: Instant::now(),
                })
            }
            GameplayCommand::CastGameObject {
                target,
                report_use: true,
                ..
            } => Some(PendingQuestAction::ControlActivation {
                target,
                started: Instant::now(),
            }),
            GameplayCommand::CastGameObject { target, .. } => Some(PendingQuestAction::Interact {
                target,
                started: Instant::now(),
            }),
            GameplayCommand::UseItemInstance {
                target: Some(target),
                ..
            } => match work {
                QuestWorkKey::InteractObjective {
                    quest, objective, ..
                } if *objective != usize::MAX => {
                    Some(self.pending_quest_credit(*quest, *objective, target, "scripted item use"))
                }
                _ => Some(PendingQuestAction::Interact {
                    target,
                    started: Instant::now(),
                }),
            },
            GameplayCommand::VehicleCast {
                target: Some(target),
                ..
            } => match work {
                QuestWorkKey::InteractObjective {
                    quest, objective, ..
                } if *objective != usize::MAX => Some(self.pending_quest_credit(
                    *quest,
                    *objective,
                    target,
                    "controlled quest ability",
                )),
                _ => Some(PendingQuestAction::Interact {
                    target,
                    started: Instant::now(),
                }),
            },
            _ => None,
        };
        if let Some(pending) = pending {
            self.dispatch_quest_semantic(command, pending).await
        } else {
            self.propose_command(command, true).await
        }
    }

    async fn dispatch_quest_semantic(
        &mut self,
        command: GameplayCommand,
        pending: PendingQuestAction,
    ) -> bool {
        if let (GameplayCommand::Loot(target), PendingQuestAction::CorpseLoot { .. }) =
            (&command, &pending)
        {
            let corpse_was_ready_to_loot = self.corpses_ready_to_loot.remove(target);
            if !corpse_was_ready_to_loot {
                let snapshot = self.snapshot_with_post_combat_corpse(
                    Snapshot::from_state(&self.state.authoritative),
                    *target,
                );
                match crate::action::spatial::is_in_range(&snapshot, &command) {
                    Some(true) => {
                        let _ = self
                            .propose_command(GameplayCommand::StopMovement, false)
                            .await;
                        self.pending_quest_action = Some(PendingQuestAction::CorpseLootDelay {
                            target: *target,
                            arrived_at: Instant::now(),
                        });
                        self.waiting(format!(
                            "reached corpse {target}; waiting one second before looting"
                        ));
                        return true;
                    }
                    None => {
                        self.waiting(format!("corpse {target} loot is waiting for authoritative mover and target positions"));
                        return true;
                    }
                    Some(false) => {}
                }
            }
            if corpse_was_ready_to_loot && self.state.authoritative.position.moving {
                let _ = self
                    .propose_command(GameplayCommand::StopMovement, false)
                    .await;
                self.pending_quest_action = Some(PendingQuestAction::CorpseLootDelay {
                    target: *target,
                    arrived_at: Instant::now(),
                });
                self.waiting(format!(
                    "reached corpse {target}; waiting for movement to stop before looting"
                ));
                return true;
            }

            if corpse_was_ready_to_loot {
                let snapshot = Snapshot::from_state(&self.state.authoritative);
                let check = crate::action::spatial::check_corpse_loot(&snapshot, *target);
                if check != crate::action::spatial::CorpseLootCheck::Ready {
                    let target_is_alive = self
                        .state
                        .authoritative
                        .entities
                        .0
                        .get(target)
                        .is_some_and(|entity| entity.health.is_some_and(|(health, _)| health > 0));
                    if matches!(
                        check,
                        crate::action::spatial::CorpseLootCheck::MissingTarget
                            | crate::action::spatial::CorpseLootCheck::NotCreature
                    ) || target_is_alive
                    {
                        self.post_combat_loot = None;
                        self.pending_quest_action = None;
                        self.corpses_ready_to_loot.remove(target);
                    }
                    if check == crate::action::spatial::CorpseLootCheck::OutOfRange {
                        self.corpses_ready_to_loot.remove(target);
                        if crate::action::spatial::movement_requirement(&snapshot, &command)
                            .is_some()
                        {
                            tracing::info!(
                                lane=?self.state.lane,
                                ?target,
                                "corpse is outside loot range; handing it to shared movement"
                            );
                        } else {
                            tracing::warn!(lane=?self.state.lane, ?target, ?check, "corpse loot request blocked by authoritative precondition check");
                            self.waiting(format!(
                                "corpse {target} is not ready to loot: {check:?}"
                            ));
                            return true;
                        }
                    } else {
                        tracing::warn!(lane=?self.state.lane, ?target, ?check, "corpse loot request blocked by authoritative precondition check");
                        self.waiting(format!("corpse {target} is not ready to loot: {check:?}"));
                        return true;
                    }
                }
            }
        }
        self.last_dispatch = DispatchOutcome::Rejected;
        if !self.propose_command(command, true).await {
            return false;
        }
        match self.last_dispatch {
            DispatchOutcome::Sent => self.pending_quest_action = Some(pending),
            DispatchOutcome::DeferredMovement => {
                if let Some(movement) = self.pending_movement.as_mut() {
                    movement.resume_pending = Some(pending);
                }
            }
            DispatchOutcome::DeferredDismount => {
                if let Some(dismount) = self.pending_dismount.as_mut() {
                    dismount.quest_action = Some(pending);
                }
            }
            _ => {}
        }
        true
    }

    async fn propose_command(&mut self, command: GameplayCommand, quest_step: bool) -> bool {
        let pet_attack = match &command {
            GameplayCommand::Attack(target) => self
                .state
                .authoritative
                .pet
                .guid
                .filter(|_| {
                    self.state
                        .authoritative
                        .pet
                        .has_active_pet(&self.state.authoritative.entities)
                        == Some(true)
                })
                .map(|pet| (pet, *target)),
            _ => None,
        };
        let action = ProposedAction {
            id: ActionId(self.next_action),
            task: TaskId(self.next_task),
            origin: PlanOrigin::SystemPolicy,
            stamp: self.state.stamp(),
            command,
        };
        self.next_action = self.next_action.wrapping_add(1).max(1);
        self.next_task = self.next_task.wrapping_add(1).max(1);
        if quest_step {
            self.last_quest_step = Some(Instant::now());
        }
        let sent = self.submit(action).await;
        if sent && let Some((pet, target)) = pet_attack {
            let pet_action = ProposedAction {
                id: ActionId(self.next_action),
                task: TaskId(self.next_task),
                origin: PlanOrigin::SystemPolicy,
                stamp: self.state.stamp(),
                command: GameplayCommand::PetAttack { pet, target },
            };
            self.next_action = self.next_action.wrapping_add(1).max(1);
            self.next_task = self.next_task.wrapping_add(1).max(1);
            let _ = self.submit(pet_action).await;
        }
        sent
    }

    fn safe_npc_approach_destination(
        &self,
        snapshot: &Snapshot,
        command: &GameplayCommand,
        fallback: Vec3,
    ) -> Option<Vec3> {
        let Some(target_id) = crate::action::spatial::npc_front_approach_target(command) else {
            return Some(fallback);
        };
        let target = crate::action::spatial::target_position(snapshot, target_id)?;
        let controller = self.movement_controller.as_ref()?;
        crate::action::spatial::first_safe_npc_approach_point(
            target,
            crate::action::spatial::NPC_FRONT_STANDOFF,
            |point| {
                controller
                    .project_grounded_waypoint(WorldPosition {
                        map: target.map,
                        point,
                        orientation: target.orientation,
                    })
                    .ok()
                    .map(|projected| projected.point)
            },
        )
    }

    fn defer_spatial_movement(
        &mut self,
        destination: Vec3,
        acceptable_range: f32,
        resume: GameplayCommand,
        origin: PlanOrigin,
        label: &'static str,
    ) {
        let work = self.current_work.clone().unwrap_or_else(|| {
            self.set_work(QuestWorkKey::TravelToObjective {
                quest: 0,
                objective: 0,
                destination: WorldPosition {
                    map: self
                        .state
                        .authoritative
                        .position
                        .player
                        .map_or(0, |position| position.map),
                    point: destination,
                    orientation: 0.0,
                },
            })
        });
        tracing::info!(lane=?self.state.lane, work_id=?work.id, ?destination, acceptable_range, resume=?resume, %label, "shared spatial precondition handed off to owned movement");
        let purpose = if origin == PlanOrigin::Recovery {
            MovementPurpose::SurvivalApproach
        } else {
            MovementPurpose::ApproachGroundedTarget
        };
        let combat_approach = crate::action::spatial::combat_approach_candidates(
            &Snapshot::from_state(&self.state.authoritative),
            &resume,
        );
        self.queue_movement(
            destination,
            acceptable_range,
            Some(resume),
            None,
            origin,
            work,
            purpose,
        );
        if let Some((candidates, combat_acceptable_range)) = combat_approach
            && let Some(movement) = self.pending_movement.as_mut()
        {
            movement.destination.point = candidates[0];
            movement.runtime.destination.point = candidates[0];
            movement.alternate_destinations = candidates.into_iter().skip(1).collect();
            movement.acceptable_range = combat_acceptable_range;
            tracing::info!(
                lane=?self.state.lane,
                work_id=?movement.work.id,
                candidates=movement.alternate_destinations.len() + 1,
                "combat approach will try bounded reachable endpoints"
            );
        }
        self.last_dispatch = DispatchOutcome::DeferredMovement;
    }

    async fn submit(&mut self, action: ProposedAction) -> bool {
        if !self.state.runnable() {
            self.last_dispatch = DispatchOutcome::Rejected;
            return true;
        }
        let original_command = action.command.clone();
        let action_origin = action.origin;
        if Self::action_requires_on_foot(&original_command)
            && (self.travel_cleanup_required()
                || (self.pending_dismount.is_some() && !self.player_is_on_foot()))
        {
            if let Some(pending) = self.pending_dismount.as_mut() {
                if pending.action.is_none()
                    || pending.action.as_ref().is_some_and(|current| {
                        current.command == original_command && current.origin == action_origin
                    })
                    || action_origin == PlanOrigin::Recovery
                {
                    pending.action = Some(action);
                    if action_origin == PlanOrigin::Recovery {
                        pending.quest_action = None;
                    }
                }
            } else {
                self.pending_dismount = Some(PendingDismount {
                    action: Some(action),
                    quest_action: None,
                    mission_revision: self.state.mission_revision,
                    player: self.authoritative_player_id(),
                    started_at: Instant::now(),
                    last_sent_at: None,
                    attempts: 0,
                });
            }
            self.last_dispatch = DispatchOutcome::DeferredDismount;
            return true;
        }
        let mut snapshot = Snapshot::from_state(&self.state.authoritative);
        if crate::action::spatial::profile(&original_command).is_some() {
            if let Some(pending) = self.pending_facing {
                if pending.started_at.elapsed() < Duration::from_secs(1) {
                    self.waiting(
                        "targeted action is waiting for authoritative facing confirmation".into(),
                    );
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                }
                tracing::warn!(lane=?self.state.lane, mover=?pending.mover, orientation=pending.orientation, "facing request had no authoritative update after one second; using the sent direction for action validation");
                if let Some(mover) = pending.mover {
                    self.assumed_facing = Some((
                        mover,
                        pending.orientation,
                        Instant::now() + Duration::from_secs(30),
                    ));
                }
                self.pending_facing = None;
            }
            if let (Some((mover, orientation, expires_at)), Some(player)) =
                (self.assumed_facing, snapshot.state.position.player.as_mut())
                && expires_at > Instant::now()
                && self.state.authoritative.control.mover.is_none()
                && self
                    .state
                    .authoritative
                    .session
                    .character_guid
                    .map(EntityId)
                    == Some(mover)
            {
                player.orientation = orientation;
            } else if self
                .assumed_facing
                .is_some_and(|(_, _, expires_at)| expires_at <= Instant::now())
            {
                self.assumed_facing = None;
            }
        }
        if let GameplayCommand::Loot(target) = &original_command {
            snapshot = self.snapshot_with_post_combat_corpse(snapshot, *target);
        }

        if let GameplayCommand::Cast { spell, .. } = &original_command
            && wow_policy::combat::spells::requires_stealth(*spell)
            && !wow_policy::combat::spells::player_has_stealth(&snapshot)
        {
            self.waiting(format!(
                "spell {spell} is waiting for an authoritative stealth aura"
            ));
            self.last_dispatch = DispatchOutcome::DeferredSpatial;
            return true;
        }

        if let GameplayCommand::Cast {
            spell,
            target: Some(target),
        } = &original_command
        {
            let key = (*spell, *target);
            let now = Instant::now();
            if self.behind_reposition_pending.remove(&key) {
                let Some(mover) = crate::action::spatial::active_mover(&snapshot) else {
                    self.behind_reposition_pending.insert(key);
                    self.waiting(
                        "rear-arc recovery is waiting for authoritative mover position".to_owned(),
                    );
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                let Some(target_position) =
                    crate::action::spatial::target_position(&snapshot, *target)
                else {
                    self.behind_reposition_pending.insert(key);
                    self.waiting(format!(
                        "rear-arc recovery is waiting for target {:?} position",
                        target
                    ));
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                let Some(requirement) =
                    crate::action::spatial::mob_behind_approach_requirement(mover, target_position)
                else {
                    self.behind_retry_cast_allowed.insert(key);
                    self.waiting(format!(
                        "rear-arc recovery cannot derive a safe position for {:?}",
                        target
                    ));
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                self.behind_retry_cast_allowed.insert(key);
                self.defer_spatial_movement(
                    requirement.destination,
                    requirement.acceptable_range,
                    original_command,
                    action_origin,
                    "server-required rear-arc reposition",
                );
                return true;
            }
            if self
                .behind_retry_after
                .get(&key)
                .is_some_and(|deadline| *deadline <= now)
            {
                self.behind_retry_after.remove(&key);
                self.behind_retry_cast_allowed.remove(&key);
            }
            if self.behind_retry_after.contains_key(&key)
                && !self.behind_retry_cast_allowed.remove(&key)
            {
                self.waiting(format!(
                    "spell {spell} is waiting for its rear-arc retry cooldown"
                ));
                self.last_dispatch = DispatchOutcome::DeferredSpatial;
                return true;
            }
        }

        if let Some(profile) = crate::action::spatial::profile(&original_command) {
            if let Some((correction, attempts)) =
                self.server_range_recovery.get(&profile.target).copied()
            {
                let Some(mover) = crate::action::spatial::active_mover(&snapshot) else {
                    self.waiting(
                        "server range recovery is waiting for authoritative mover position"
                            .to_owned(),
                    );
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                let Some(target_position) =
                    crate::action::spatial::target_position(&snapshot, profile.target)
                else {
                    self.waiting(format!(
                        "server range recovery is waiting for target {:?} position",
                        profile.target
                    ));
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                let attempt = attempts.saturating_sub(1);
                let Some(destination) = crate::action::spatial::server_range_reposition(
                    mover,
                    target_position,
                    correction,
                    attempt,
                ) else {
                    self.waiting(format!(
                        "server range recovery cannot derive a safe reposition for {:?}",
                        profile.target
                    ));
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                self.server_range_recovery.remove(&profile.target);
                self.defer_spatial_movement(
                    destination,
                    0.75,
                    original_command,
                    action_origin,
                    "server range recovery",
                );
                return true;
            }
        }

        // Normal deterministic range comes before LOS and facing.
        let normal_range_blocked =
            crate::action::spatial::movement_requirement(&snapshot, &original_command).is_some();
        if !normal_range_blocked {
            if let Some(requirement) =
                crate::action::spatial::line_of_sight_requirement(&original_command, |target| {
                    if self.los_blocked.contains(&target) {
                        crate::action::spatial::LineOfSightStatus::Blocked
                    } else {
                        crate::action::spatial::LineOfSightStatus::Unknown
                    }
                })
            {
                let Some(mover) = crate::action::spatial::active_mover(&snapshot) else {
                    self.waiting(
                        "line-of-sight recovery is waiting for authoritative mover position"
                            .to_owned(),
                    );
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                let Some(target_position) =
                    crate::action::spatial::target_position(&snapshot, requirement.target)
                else {
                    self.waiting(format!(
                        "line-of-sight recovery is waiting for target {:?} position",
                        requirement.target
                    ));
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                let attempt = self
                    .los_attempts
                    .get(&requirement.target)
                    .copied()
                    .unwrap_or(1)
                    .saturating_sub(1);
                let Some(destination) = crate::action::spatial::line_of_sight_reposition(
                    mover,
                    target_position,
                    attempt,
                ) else {
                    self.waiting(format!(
                        "line-of-sight recovery cannot derive a safe reposition for {:?}",
                        requirement.target
                    ));
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                self.los_blocked.remove(&requirement.target);
                self.defer_spatial_movement(
                    destination,
                    0.75,
                    original_command,
                    action_origin,
                    "line-of-sight recovery",
                );
                return true;
            }
        }
        match finalize(
            &snapshot,
            self.state.stamp(),
            self.state.activation,
            self.state.mission.permissions,
            &self.runtime_tuning.maintenance.bank_keep_item_ids,
            self.runtime_tuning.maintenance.auto_professions_enabled,
            self.battleground_pvp_authorized(),
            &self.fresh_group_loot_rolls(Instant::now()),
            action,
        ) {
            ValidationOutcome::Sendable(action) => {
                let repair_request =
                    matches!(original_command, GameplayCommand::RepairEquipment { .. });
                tracing::info!(lane=?self.state.lane, action=?action.id(), command=?action.command(), "validated gameplay action queued for proxy");
                let sent = self.proxy.send(WorkerToProxy::Action(action)).await.is_ok();
                self.last_dispatch = if sent {
                    DispatchOutcome::Sent
                } else {
                    DispatchOutcome::TransportClosed
                };
                if sent && repair_request {
                    self.repair_pending = Some((
                        self.state.authoritative.inventory.equipment_condition,
                        Instant::now(),
                    ));
                    self.repair_retry_after = None;
                }
                sent
            }
            ValidationOutcome::NeedsMovement(requirement) => {
                let Some(destination) = self.safe_npc_approach_destination(
                    &snapshot,
                    &original_command,
                    requirement.destination,
                ) else {
                    self.waiting(
                        "NPC approach has no navigation-validated safe ground point".to_owned(),
                    );
                    self.last_dispatch = DispatchOutcome::DeferredSpatial;
                    return true;
                };
                self.defer_spatial_movement(
                    destination,
                    requirement.acceptable_range,
                    original_command,
                    action_origin,
                    "range precondition",
                );
                true
            }
            ValidationOutcome::NeedsFacing(requirement) => {
                self.dispatch_facing_precondition(
                    &snapshot,
                    requirement,
                    action_origin,
                    &original_command,
                )
                .await
            }
            ValidationOutcome::NeedsLineOfSight(requirement) => {
                self.los_blocked.insert(requirement.target);
                self.last_dispatch = DispatchOutcome::DeferredSpatial;
                true
            }
            ValidationOutcome::Rejected(failure) => {
                tracing::warn!(lane=?self.state.lane, code=%failure.code, message=%failure.message, retryable=failure.retryable, "gameplay action rejected by final validator");
                self.last_dispatch = DispatchOutcome::Rejected;
                true
            }
        }
    }

    /// Turn the active mover toward the target before retrying its action.
    async fn dispatch_facing_precondition(
        &mut self,
        snapshot: &Snapshot,
        requirement: FacingRequirement,
        action_origin: PlanOrigin,
        original_command: &GameplayCommand,
    ) -> bool {
        let face = ProposedAction {
            id: ActionId(self.next_action),
            task: TaskId(self.next_task),
            origin: action_origin,
            stamp: self.state.stamp(),
            command: GameplayCommand::FaceDirection {
                orientation: requirement.orientation,
            },
        };
        self.next_action = self.next_action.wrapping_add(1).max(1);
        self.next_task = self.next_task.wrapping_add(1).max(1);

        match finalize(
            snapshot,
            self.state.stamp(),
            self.state.activation,
            self.state.mission.permissions,
            &self.runtime_tuning.maintenance.bank_keep_item_ids,
            self.runtime_tuning.maintenance.auto_professions_enabled,
            self.battleground_pvp_authorized(),
            &self.fresh_group_loot_rolls(Instant::now()),
            face,
        ) {
            ValidationOutcome::Sendable(face) => {
                tracing::info!(lane=?self.state.lane, orientation=requirement.orientation, tolerance=requirement.tolerance, resume=?original_command, "shared facing precondition queued before targeted action");
                let sent = self.proxy.send(WorkerToProxy::Action(face)).await.is_ok();
                if sent {
                    self.pending_facing = Some(PendingFacing {
                        mover: self.state.authoritative.control.mover.or_else(|| {
                            self.state
                                .authoritative
                                .session
                                .character_guid
                                .map(EntityId)
                        }),
                        orientation: requirement.orientation,
                        tolerance: requirement.tolerance,
                        started_at: Instant::now(),
                    });
                }
                self.last_dispatch = if sent {
                    DispatchOutcome::DeferredSpatial
                } else {
                    DispatchOutcome::TransportClosed
                };
                sent
            }
            other => {
                tracing::warn!(lane=?self.state.lane, ?other, "shared facing precondition failed validation");
                self.last_dispatch = DispatchOutcome::Rejected;
                true
            }
        }
    }

    fn waiting(&mut self, reason: String) {
        if self.last_wait_reason.as_deref() != Some(reason.as_str()) {
            tracing::info!(lane=?self.state.lane, reason=%reason, "mission scheduler waiting");
            self.last_wait_reason = Some(reason);
        }
    }

    fn reset_session_work(&mut self) {
        self.group_pull_delay = None;
        self.last_quest_step = None;
        self.pending_accept = None;
        self.pending_giver_interaction = None;
        self.giver_retry_after.clear();
        self.pending_turn_in = None;
        self.reward_metadata_waiting = None;
        self.last_turn_in_search_query = None;
        self.turn_in_search_failures.clear();
        self.cancel_route_job();
        self.pending_movement = None;
        self.pending_facing = None;
        self.pending_quest_action = None;
        self.search_attempts.clear();
        self.quest_start_search_attempts.clear();
        self.search_retry_after.clear();
        self.combat_target_retry_after.clear();
        self.search_roam_cursor.clear();
        self.current_work = None;
        self.last_wait_reason = None;
        self.last_dispatch = DispatchOutcome::Rejected;
        self.credited_quest_targets.clear();
        self.loot_retry_after.clear();
        self.bag_full_loot_targets.clear();
        self.queried_item_templates.clear();
        self.looted_corpses.clear();
        self.corpses_ready_to_loot.clear();
        self.los_blocked.clear();
        self.los_attempts.clear();
        self.server_range_recovery.clear();
        self.behind_reposition_pending.clear();
        self.behind_retry_after.clear();
        self.behind_retry_cast_allowed.clear();
        self.maintenance_retry_after.clear();
        self.active_player_recovery = None;
        self.recovery_vendor_buy_pending = None;
        self.repair_pending = None;
        self.repair_retry_after = None;
        self.remembered_class_trainers.clear();
        self.remembered_bankers.clear();
        self.bank_travel_retry_after = None;
        self.bank_open_pending = None;
        self.bank_deposit_pending = None;
        self.bank_retry_after = None;
        self.class_training_character = None;
        self.last_player_level = None;
        self.class_training_due = false;
        self.class_trainer_travel_retry_after = None;
        self.last_maintenance_tick = None;
        self.last_maintenance_status = None;
        self.post_combat_loot = None;
        self.last_recovery_action = None;
        self.last_logged_player_life_status = None;
        self.last_survival_action = None;
    }
}

const CORPSE_STALE_TARGET_DISTANCE: f32 = 6.0;

fn refresh_cached_post_combat_corpse(
    queued: &mut Option<(EntityId, Instant, Option<wow_state::entities::EntityState>)>,
    target: EntityId,
    observed: &wow_state::entities::EntityState,
) -> Option<WorldPosition> {
    if observed.health.is_some_and(|(current, _)| current > 0) {
        return None;
    }
    let Some((queued_target, _, cached)) = queued else {
        return None;
    };
    if *queued_target != target {
        return None;
    }
    let mut current = observed.clone();
    current.mark_dead();
    if let (Some(previous), Some(observed_position)) = (cached.as_ref(), current.position)
        && ((current.target.is_some() && current.target == previous.target)
            || (current.target.is_none() && previous.target.is_some()))
        && let Some(previous_position) = previous.position
        && previous_position.map == observed_position.map
        && previous_position.point.distance(observed_position.point) > CORPSE_STALE_TARGET_DISTANCE
    {
        // Keep the death-site correction when a later object update restores
        // the creature's stale spawn position. A death update can also clear
        // the target field, so an absent target does not invalidate the cached
        // correction.
        current.position = Some(previous_position);
    }
    let position = current.position;
    *cached = Some(current);
    position
}

fn current_loot_approach(snapshot: &Snapshot, target: EntityId) -> Option<(WorldPosition, f32)> {
    let target_position = crate::action::spatial::target_position(snapshot, target)?;
    let requirement =
        crate::action::spatial::movement_requirement(snapshot, &GameplayCommand::Loot(target))?;
    Some((
        WorldPosition {
            map: target_position.map,
            point: requirement.destination,
            orientation: target_position.orientation,
        },
        requirement.acceptable_range,
    ))
}

/// Replace an old creature location with the nearby unit it was attacking.
/// WotLK creature movement packets can leave the object's stored position at
/// its spawn point while the creature runs toward the player or pet.
fn relocate_moved_mob_corpse(
    state: &wow_state::AuthoritativeState,
    corpse: &mut wow_state::entities::EntityState,
) -> bool {
    let Some(attacker) = corpse.target else {
        return false;
    };
    let player = state.session.character_guid.map(EntityId);
    let player_is_attacker = player == Some(attacker);
    let pet_is_attacker = state.pet.guid == Some(attacker);
    if !player_is_attacker && !pet_is_attacker {
        return false;
    }

    let attacker_position = if player_is_attacker {
        state
            .control
            .active_position(state.position.player)
            .or_else(|| {
                state
                    .entities
                    .0
                    .get(&attacker)
                    .and_then(|entity| entity.position)
            })
    } else {
        state
            .entities
            .0
            .get(&attacker)
            .and_then(|entity| entity.position)
    };
    let (Some(corpse_position), Some(attacker_position)) = (corpse.position, attacker_position)
    else {
        return false;
    };
    if corpse_position.map != attacker_position.map
        || !corpse_position.point.is_finite()
        || !attacker_position.point.is_finite()
        || corpse_position.point.distance(attacker_position.point) <= CORPSE_STALE_TARGET_DISTANCE
    {
        return false;
    }

    corpse.position = Some(attacker_position);
    true
}

fn movement_step_distance(run_speed: f32, last_step: Option<Instant>, now: Instant) -> f32 {
    let elapsed = last_step
        .map(|at| now.saturating_duration_since(at))
        .unwrap_or(MOVEMENT_STEP_INTERVAL)
        .clamp(MOVEMENT_STEP_INTERVAL, Duration::from_millis(250));
    run_speed * elapsed.as_secs_f32()
}

fn nearest_visible_gameobject(
    entities: &BTreeMap<EntityId, wow_state::entities::EntityState>,
    entry: u32,
    map: u32,
    from: Vec3,
) -> Option<EntityId> {
    entities
        .values()
        .filter(|entity| {
            entity.entry == entry && entity.kind == wow_state::entities::EntityKind::GameObject
        })
        .filter_map(|entity| {
            entity
                .position
                .filter(|position| position.map == map)
                .map(|position| (entity.id, position.point))
        })
        .min_by(|(_, left), (_, right)| left.distance(from).total_cmp(&right.distance(from)))
        .map(|(id, _)| id)
}

fn corpse_reclaim_is_safe(
    entities: &BTreeMap<EntityId, wow_state::entities::EntityState>,
    player: EntityId,
    corpse: WorldPosition,
) -> bool {
    !entities.iter().any(|(id, entity)| {
        *id != player
            && entity.hostile
            && !entity.is_dead()
            && entity.position.is_some_and(|position| {
                position.map == corpse.map
                    && position.point.distance(corpse.point) < CORPSE_HOSTILE_CLEARANCE_YARDS
            })
    })
}

fn corpse_reclaim_attempt_allowed(attempts: u8) -> bool {
    attempts < MAX_CORPSE_RECLAIM_ATTEMPTS
}

fn corpse_route_map_matches(player: WorldPosition, corpse: WorldPosition) -> bool {
    player.map == corpse.map
}

fn quest_status_available(status: u8) -> bool {
    matches!(status, 2 | 4 | 7 | 8)
}
fn quest_status_reward(status: u8) -> bool {
    matches!(status, 3 | 6 | 9 | 10)
}
fn quest_confirmation_waiting(started: Instant, now: Instant) -> bool {
    now.saturating_duration_since(started) < QUEST_CONFIRMATION_TIMEOUT
}
fn giver_retry_delay(attempts: u8) -> Duration {
    Duration::from_secs(
        5_u64
            .saturating_mul(1_u64 << attempts.saturating_sub(1).min(3))
            .min(40),
    )
}

fn bounded_candidates(candidates: Vec<Vec3>, origin: Vec3) -> Vec<Vec3> {
    bounded_candidates_within(candidates, origin, 250.0)
}

fn bounded_candidates_within(
    mut candidates: Vec<Vec3>,
    origin: Vec3,
    max_distance: f32,
) -> Vec<Vec3> {
    candidates.retain(|point| point.is_finite() && point.distance(origin) <= max_distance);
    candidates.sort_by(|a, b| origin.distance(*a).total_cmp(&origin.distance(*b)));
    candidates.dedup();
    candidates.truncate(5);
    candidates
}

fn is_terminal_navigation_failure(error: &wow_navigation::NavigationError) -> bool {
    matches!(
        error,
        wow_navigation::NavigationError::InvalidCoordinate
            | wow_navigation::NavigationError::MissingNavigationData
            | wow_navigation::NavigationError::NoRoute
            | wow_navigation::NavigationError::FloorDiscontinuity
            | wow_navigation::NavigationError::RetryExhausted
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MovementFailure {
    TimedOut,
    Stalled,
}

fn movement_failure(
    movement_elapsed: Duration,
    no_progress_elapsed: Duration,
) -> Option<MovementFailure> {
    if movement_elapsed >= MOVEMENT_OPERATION_TIMEOUT {
        Some(MovementFailure::TimedOut)
    } else if no_progress_elapsed >= MOVEMENT_STALL_TIMEOUT {
        Some(MovementFailure::Stalled)
    } else {
        None
    }
}

fn search_point_key(point: Vec3) -> (u32, u32, u32) {
    (point.x.to_bits(), point.y.to_bits(), point.z.to_bits())
}
fn quest_start_search_point_key(map: u32, point: Vec3) -> (u32, u32, u32, u32) {
    let (x, y, z) = search_point_key(point);
    (map, x, y, z)
}
fn turn_in_search_arrived(player: Vec3, destination: Vec3) -> bool {
    player.distance(destination) <= TURN_IN_SEARCH_RANGE
}
fn quest_search_arrived(player: Vec3, destination: Vec3) -> bool {
    player.distance(destination) <= QUEST_SEARCH_ARRIVAL_RANGE
}
fn quest_tool_search_arrived(player: Vec3, destination: Vec3) -> bool {
    player.distance(destination) <= QUEST_TOOL_SEARCH_RANGE
}
fn is_quest_search_work(movement: &PendingMovement) -> bool {
    movement.purpose == MovementPurpose::SearchArea
        && matches!(
            &movement.work.key,
            QuestWorkKey::TravelToObjective { .. } | QuestWorkKey::CollectItem { .. }
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn low_health_recovery_state() -> wow_state::AuthoritativeState {
        let player = EntityId(1);
        let mut state = wow_state::AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(player.0);
        state.capabilities.class_id = Some(1);
        state.auras.by_entity.insert(player, Default::default());
        state.inventory.instances_authoritative = true;
        state.inventory.instances.insert(
            EntityId(2),
            wow_state::inventory::InventoryItemInstance {
                item: 100,
                guid: EntityId(2),
                backpack_slot: 23,
                count: 1,
            },
        );
        state.inventory.item_metadata.insert(
            100,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Conjured Bread".into(),
                item_class: 0,
                subclass: 5,
                use_spell_id: 200,
                allowable_class: 0,
                required_level: 1,
                ..Default::default()
            },
        );
        state.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                kind: wow_state::entities::EntityKind::Player,
                health: Some((50, 100)),
                level: Some(10),
                unit_flags: Some(0),
                ..Default::default()
            },
        );
        state
    }

    #[tokio::test]
    async fn player_recovery_dispatches_shared_item_command_and_waits_for_server_progress() {
        let mut engine = test_engine(Mission::quest(MissionId(1)), low_health_recovery_state()).0;
        let (proxy_tx, mut proxy) = mpsc::channel(4);
        engine.proxy = proxy_tx;

        assert!(engine.tick_maintenance().await.unwrap());
        let WorkerToProxy::Action(action) = proxy.try_recv().expect("recovery item action") else {
            panic!("expected a typed item action")
        };
        assert_eq!(
            action.command(),
            &GameplayCommand::UseItemInstance {
                item: 100,
                item_guid: EntityId(2),
                backpack_slot: 23,
                spell: 200,
                target: Some(EntityId(1)),
                cast_count: 0,
            }
        );
        assert!(engine.active_player_recovery.is_some());

        engine
            .state
            .authoritative
            .auras
            .by_entity
            .get_mut(&EntityId(1))
            .unwrap()
            .insert(
                0,
                wow_state::auras::AuraInstance {
                    slot: 0,
                    spell: 200,
                    positive: Some(true),
                    caster: Some(EntityId(1)),
                    max_duration_ms: None,
                    remaining_ms: None,
                    observed_at_ms: None,
                },
            );
        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert!(engine.tick_active_player_recovery(&snapshot, Instant::now()));
        assert!(engine.active_player_recovery.as_ref().unwrap().aura_seen);

        engine
            .state
            .authoritative
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .health = Some((80, 100));
        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert!(engine.tick_active_player_recovery(&snapshot, Instant::now()));
        assert!(engine.active_player_recovery.is_some());

        engine
            .state
            .authoritative
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .health = Some((90, 100));
        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert!(!engine.tick_active_player_recovery(&snapshot, Instant::now()));
        assert!(engine.active_player_recovery.is_none());
    }

    #[tokio::test]
    async fn player_recovery_uses_authoritative_drink_for_low_mana() {
        let mut state = low_health_recovery_state();
        let player = state.entities.0.get_mut(&EntityId(1)).unwrap();
        player.health = Some((100, 100));
        player.power = Some((20, 100));
        player.power_type = Some(0);
        state.inventory.instances.clear();
        state.inventory.item_metadata.clear();
        state.inventory.instances.insert(
            EntityId(3),
            wow_state::inventory::InventoryItemInstance {
                item: 101,
                guid: EntityId(3),
                backpack_slot: 24,
                count: 1,
            },
        );
        state.inventory.item_metadata.insert(
            101,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Conjured Water".into(),
                item_class: 0,
                subclass: 5,
                use_spell_id: 201,
                allowable_class: 0,
                required_level: 1,
                ..Default::default()
            },
        );
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(1)), state);

        assert!(engine.tick_maintenance().await.unwrap());
        let WorkerToProxy::Action(action) = proxy.try_recv().expect("drink item action") else {
            panic!("expected a typed drink action")
        };
        assert_eq!(
            action.command(),
            &GameplayCommand::UseItemInstance {
                item: 101,
                item_guid: EntityId(3),
                backpack_slot: 24,
                spell: 201,
                target: Some(EntityId(1)),
                cast_count: 0,
            }
        );
        assert_eq!(
            engine
                .active_player_recovery
                .as_ref()
                .unwrap()
                .selection
                .kind,
            wow_policy::maintenance::PlayerRecoveryKind::Drink
        );
    }

    #[test]
    fn combat_preempts_player_recovery_and_arms_bounded_retry() {
        let mut engine = test_engine(Mission::quest(MissionId(1)), low_health_recovery_state()).0;
        let now = Instant::now();
        let selection = wow_policy::maintenance::select_player_recovery_item(
            &Snapshot::from_state(&engine.state.authoritative),
        )
        .unwrap();
        engine.active_player_recovery = Some(ActivePlayerRecovery {
            selection: selection.clone(),
            started_at: now,
            deadline: now + PLAYER_RECOVERY_MAX,
            last_progress_at: now,
            last_resource: 50,
            aura_seen: false,
        });
        engine
            .state
            .authoritative
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .unit_flags = Some(0x0008_0000);

        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert!(!engine.tick_active_player_recovery(&snapshot, now));
        assert!(engine.active_player_recovery.is_none());
        assert_eq!(
            engine
                .maintenance_retry_after
                .get(&(selection.spell, selection.player)),
            Some(&(now + PLAYER_RECOVERY_RETRY))
        );
    }

    #[test]
    fn recovery_vendor_purchase_wait_is_bounded_and_tracks_inventory_changes() {
        let now = Instant::now();
        let deadline = now + RECOVERY_VENDOR_BUY_PENDING_TIMEOUT;
        assert!(!recovery_vendor_buy_pending_done(4, 4, deadline, now));
        assert!(recovery_vendor_buy_pending_done(4, 5, deadline, now));
        assert!(recovery_vendor_buy_pending_done(4, 4, deadline, deadline));
    }

    #[test]
    fn health_and_mana_potions_share_a_one_hour_fallback_lockout() {
        let now = Instant::now();
        let lockout = now + COMBAT_POTION_FALLBACK_LOCKOUT;
        assert!(!combat_potion_lockout_ready(Some(lockout), now));
        assert!(combat_potion_lockout_ready(Some(lockout), lockout));
        assert!(combat_potion_lockout_ready(None, now));
        assert_eq!(COMBAT_POTION_FALLBACK_LOCKOUT, Duration::from_secs(3_600));
    }

    #[test]
    fn mail_action_wait_suppresses_duplicates_until_state_changes_or_timeout() {
        let now = Instant::now();
        let deadline = now + MAILBOX_ACTION_TIMEOUT;
        assert!(waiting_for_mail_observation(9, 9, deadline, now));
        assert!(!waiting_for_mail_observation(9, 10, deadline, now));
        assert!(!waiting_for_mail_observation(9, 9, deadline, deadline));
    }

    #[test]
    fn survival_item_retry_windows_are_independent_of_the_potion_lockout() {
        let now = Instant::now();
        assert_eq!(COMBAT_HEALTHSTONE_RETRY, Duration::from_secs(5));
        assert_eq!(COMBAT_EMERGENCY_HEALTH_ITEM_RETRY, Duration::from_secs(30));
        assert!(!combat_item_retry_ready(
            Some(now + COMBAT_HEALTHSTONE_RETRY),
            now
        ));
        assert!(combat_item_retry_ready(
            Some(now + COMBAT_HEALTHSTONE_RETRY),
            now + COMBAT_HEALTHSTONE_RETRY
        ));
        assert!(combat_potion_lockout_ready(None, now));
        assert!(!combat_potion_lockout_ready(
            Some(now + COMBAT_POTION_FALLBACK_LOCKOUT),
            now
        ));
    }
    use crate::activity::ActivityArbiter;

    #[test]
    fn group_follow_spacing_uses_party_and_raid_roles() {
        for (intent, expected) in [
            (
                MissionIntent::Party {
                    role: GroupRole::Tank,
                },
                4.0,
            ),
            (
                MissionIntent::Raid {
                    role: GroupRole::Healer,
                },
                20.0,
            ),
            (
                MissionIntent::Party {
                    role: GroupRole::Ranged,
                },
                17.0,
            ),
            (
                MissionIntent::Raid {
                    role: GroupRole::Melee,
                },
                5.0,
            ),
            (MissionIntent::Idle, 17.0),
        ] {
            assert_eq!(group_follow_stop_distance(&intent), expected);
        }
    }

    fn group_engagement_fixture() -> (LaneEngine, Snapshot, EntityId) {
        let player = EntityId(1);
        let member = EntityId(2);
        let target = EntityId(9);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        authoritative.group.lifecycle = wow_state::group::GroupLifecycle::Active;
        authoritative.group.generation = 4;
        authoritative
            .group
            .members
            .push(wow_state::group::GroupMember {
                entity: member,
                online: true,
                ..Default::default()
            });
        authoritative.group.encounter_target = Some(target);
        authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                hostile: true,
                health: Some((100, 100)),
                target: Some(member),
                unit_flags: Some(0x0008_0000),
                ..Default::default()
            },
        );
        let (engine, _) = test_engine(
            Mission {
                id: MissionId(92),
                intent: MissionIntent::Party {
                    role: GroupRole::Melee,
                },
                permissions: PermissionSet::MOVE | PermissionSet::COMBAT,
            },
            authoritative,
        );
        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        (engine, snapshot, target)
    }

    #[test]
    fn group_pull_timer_starts_on_observed_engagement_and_is_target_scoped() {
        let (mut engine, snapshot, target) = group_engagement_fixture();
        let now = Instant::now();
        assert!(!engine.group_pull_delay_elapsed(
            &snapshot,
            GroupRole::Melee,
            Some(EntityId(1)),
            target,
            now,
        ));
        assert!(engine.group_pull_delay.is_some());

        engine.group_pull_delay = Some(GroupPullDelay {
            target,
            group_generation: snapshot.state.group.generation,
            map: Some(1),
            started_at: now - Duration::from_millis(1_501),
        });
        assert!(engine.group_pull_delay_elapsed(
            &snapshot,
            GroupRole::Melee,
            Some(EntityId(1)),
            target,
            now,
        ));

        let other_target = EntityId(10);
        let mut changed = snapshot.clone();
        changed.state.group.encounter_target = Some(other_target);
        changed.state.entities.0.insert(
            other_target,
            wow_state::entities::EntityState {
                id: other_target,
                hostile: true,
                target: Some(EntityId(2)),
                unit_flags: Some(0x0008_0000),
                ..Default::default()
            },
        );
        assert!(!engine.group_pull_delay_elapsed(
            &changed,
            GroupRole::Melee,
            Some(EntityId(1)),
            other_target,
            now,
        ));
        assert_eq!(engine.group_pull_delay.unwrap().target, other_target);

        engine.group_pull_delay.as_mut().unwrap().started_at = now - Duration::from_millis(1_501);
        let mut new_group = changed.clone();
        new_group.state.group.generation += 1;
        assert!(!engine.group_pull_delay_elapsed(
            &new_group,
            GroupRole::Melee,
            Some(EntityId(1)),
            other_target,
            now,
        ));
        assert_eq!(
            engine.group_pull_delay.unwrap().group_generation,
            new_group.state.group.generation
        );

        engine.group_pull_delay.as_mut().unwrap().started_at = now - Duration::from_millis(1_501);
        let mut new_map = new_group.clone();
        new_map.state.position.player.as_mut().unwrap().map = 2;
        assert!(!engine.group_pull_delay_elapsed(
            &new_map,
            GroupRole::Melee,
            Some(EntityId(1)),
            other_target,
            now,
        ));
        assert_eq!(engine.group_pull_delay.unwrap().map, Some(2));
    }

    #[test]
    fn group_pull_timer_clears_when_engagement_is_lost_or_tank_takes_over() {
        let (mut engine, snapshot, target) = group_engagement_fixture();
        let now = Instant::now();
        engine.group_pull_delay_elapsed(
            &snapshot,
            GroupRole::Melee,
            Some(EntityId(1)),
            target,
            now,
        );

        let mut disengaged = snapshot.clone();
        disengaged
            .state
            .entities
            .0
            .get_mut(&target)
            .unwrap()
            .unit_flags = Some(0);
        assert!(!engine.group_pull_delay_elapsed(
            &disengaged,
            GroupRole::Melee,
            Some(EntityId(1)),
            target,
            now,
        ));
        assert!(engine.group_pull_delay.is_none());

        engine.group_pull_delay_elapsed(
            &snapshot,
            GroupRole::Melee,
            Some(EntityId(1)),
            target,
            now,
        );
        assert!(engine.group_pull_delay_elapsed(
            &snapshot,
            GroupRole::Tank,
            Some(EntityId(1)),
            target,
            now,
        ));
        assert!(engine.group_pull_delay.is_none());
    }

    #[tokio::test]
    async fn group_pull_timer_clears_on_world_and_mission_changes() {
        let (mut engine, _, target) = group_engagement_fixture();
        let timer = || GroupPullDelay {
            target,
            group_generation: 4,
            map: Some(1),
            started_at: Instant::now(),
        };
        engine.group_pull_delay = Some(timer());
        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::WorldChanged {
                    character_guid: 1,
                    position: WorldPosition {
                        map: 2,
                        point: Vec3::new(10.0, 10.0, 0.0),
                        orientation: 0.0,
                    },
                },
            ))
            .await;
        assert!(engine.group_pull_delay.is_none());

        engine.group_pull_delay = Some(timer());
        engine
            .handle(LaneMessage::ReplaceMission(Mission::idle()))
            .await;
        assert!(engine.group_pull_delay.is_none());
    }

    #[test]
    fn rogue_restock_vendor_must_be_a_live_cataloged_seller_within_interaction_range() {
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let seller = catalog
            .world()
            .vendor_services
            .iter()
            .find(|service| service.can_sell && !service.spawns.is_empty())
            .expect("catalog contains a seller");
        let spawn = &seller.spawns[0];
        let position = WorldPosition {
            map: spawn.map_id,
            point: Vec3::new(spawn.x, spawn.y, spawn.z),
            orientation: 0.0,
        };
        let mut state = wow_state::AuthoritativeState::default();
        state.session.character_guid = Some(7);
        state.position.player = Some(position);
        let vendor = EntityId(55);
        state.entities.0.insert(
            vendor,
            wow_state::entities::EntityState {
                id: vendor,
                entry: seller.entry_id,
                kind: wow_state::entities::EntityKind::Unit,
                interactable: true,
                position: Some(position),
                ..Default::default()
            },
        );
        assert_eq!(
            nearby_sell_vendor(&Snapshot::from_state(&state)),
            Some(vendor)
        );

        state.entities.0.get_mut(&vendor).unwrap().position = Some(WorldPosition {
            point: Vec3::new(position.point.x + 6.0, position.point.y, position.point.z),
            ..position
        });
        assert_eq!(nearby_sell_vendor(&Snapshot::from_state(&state)), None);
    }

    #[test]
    fn class_trainer_selection_requires_observed_class_service_flags_and_range() {
        let position = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let trainer = EntityId(55);
        let mut state = wow_state::AuthoritativeState::default();
        state.position.player = Some(position);
        state.entities.0.insert(
            trainer,
            wow_state::entities::EntityState {
                id: trainer,
                kind: wow_state::entities::EntityKind::Unit,
                interactable: true,
                npc_flags: Some(0x30),
                position: Some(position),
                ..Default::default()
            },
        );
        assert_eq!(
            nearby_class_trainer(&Snapshot::from_state(&state)),
            Some(trainer)
        );

        state.entities.0.get_mut(&trainer).unwrap().npc_flags = Some(0x50);
        assert_eq!(nearby_class_trainer(&Snapshot::from_state(&state)), None);
        state.entities.0.get_mut(&trainer).unwrap().npc_flags = Some(0x30);
        state.entities.0.get_mut(&trainer).unwrap().position = Some(WorldPosition {
            point: Vec3::new(6.0, 0.0, 0.0),
            ..position
        });
        assert_eq!(nearby_class_trainer(&Snapshot::from_state(&state)), None);
    }

    #[test]
    fn remembered_class_trainer_requires_recent_same_map_location_within_detour_radius() {
        let now = Instant::now();
        let player_position = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let destination = WorldPosition {
            point: Vec3::new(29.0, 0.0, 0.0),
            ..player_position
        };
        let mut state = wow_state::AuthoritativeState::default();
        state.position.player = Some(player_position);
        let snapshot = Snapshot::from_state(&state);
        let memory = BTreeMap::from([(1, (destination, now))]);
        assert_eq!(
            remembered_class_trainer_destination(&snapshot, &memory, now),
            Some(destination)
        );

        let far = WorldPosition {
            point: Vec3::new(31.0, 0.0, 0.0),
            ..player_position
        };
        assert_eq!(
            remembered_class_trainer_destination(
                &snapshot,
                &BTreeMap::from([(1, (far, now))]),
                now
            ),
            None
        );
        assert_eq!(
            remembered_class_trainer_destination(
                &snapshot,
                &BTreeMap::from([(2, (destination, now))]),
                now
            ),
            None
        );
        assert_eq!(
            remembered_class_trainer_destination(
                &snapshot,
                &memory,
                now + CLASS_TRAINER_MEMORY_MAX_AGE + Duration::from_secs(1)
            ),
            None
        );
    }

    #[test]
    fn trainer_travel_obeys_permission_group_money_combat_and_quest_gates() {
        let position = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let player = EntityId(1);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(position);
        authoritative.inventory.money = CLASS_TRAINER_MONEY_RESERVE_COPPER + 1;
        authoritative.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                kind: wow_state::entities::EntityKind::Player,
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        let (mut engine, _) = test_engine(Mission::quest(MissionId(1)), authoritative);
        engine.class_training_due = true;
        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert!(engine.class_trainer_travel_allowed(&snapshot));

        engine.state.mission.permissions = PermissionSet::MAINTENANCE;
        assert!(!engine.class_trainer_travel_allowed(&snapshot));
        engine.state.mission.permissions = PermissionSet::MAINTENANCE | PermissionSet::MOVE;
        engine.state.authoritative.group.lifecycle = wow_state::group::GroupLifecycle::Active;
        assert!(!engine.class_trainer_travel_allowed(&snapshot));
        engine.state.authoritative.group.lifecycle = wow_state::group::GroupLifecycle::Solo;
        engine.state.authoritative.inventory.money = CLASS_TRAINER_MONEY_RESERVE_COPPER;
        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert!(!engine.class_trainer_travel_allowed(&snapshot));
        engine.state.authoritative.inventory.money = CLASS_TRAINER_MONEY_RESERVE_COPPER + 1;
        engine
            .state
            .authoritative
            .entities
            .0
            .get_mut(&player)
            .unwrap()
            .unit_flags = Some(0x0008_0000);
        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert!(!engine.class_trainer_travel_allowed(&snapshot));
        engine
            .state
            .authoritative
            .entities
            .0
            .get_mut(&player)
            .unwrap()
            .unit_flags = Some(0);
        let giver = EntityId(22);
        engine.state.authoritative.entities.0.insert(
            giver,
            wow_state::entities::EntityState {
                id: giver,
                position: Some(WorldPosition {
                    point: Vec3::new(39.0, 0.0, 0.0),
                    ..position
                }),
                ..Default::default()
            },
        );
        engine
            .state
            .authoritative
            .quests
            .offers
            .insert(99, wow_state::quests::QuestOffer { giver, icon: 0 });
        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert!(!engine.class_trainer_travel_allowed(&snapshot));
    }

    #[test]
    fn trainer_travel_queues_short_detour_without_replacing_mission() {
        let position = WorldPosition {
            map: 4,
            point: Vec3::new(10.0, 10.0, 5.0),
            orientation: 0.0,
        };
        let player = EntityId(1);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(position);
        authoritative.inventory.money = CLASS_TRAINER_MONEY_RESERVE_COPPER + 1;
        authoritative.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                kind: wow_state::entities::EntityKind::Player,
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        let mission = Mission::quest(MissionId(88));
        let intent = mission.intent.clone();
        let (mut engine, _) = test_engine(mission, authoritative);
        engine.class_training_due = true;
        let destination = WorldPosition {
            point: Vec3::new(20.0, 10.0, 5.0),
            ..position
        };
        engine
            .remembered_class_trainers
            .insert(4, (destination, Instant::now()));
        let snapshot = Snapshot::from_state(&engine.state.authoritative);

        assert_eq!(engine.tick_class_trainer_travel(&snapshot), Some(true));
        assert_eq!(engine.state.mission.intent, intent);
        assert!(engine.pending_movement.as_ref().is_some_and(|movement| {
            movement.purpose == MovementPurpose::ClassTrainer && movement.destination == destination
        }));
    }

    #[test]
    fn profession_trainer_travel_uses_a_nearby_catalog_hint_and_preserves_mission() {
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let service = catalog
            .world()
            .trainer_services
            .iter()
            .find(|service| {
                service
                    .skills
                    .contains(&wow_policy::gathering::professions::COOKING)
                    && !service.spawns.is_empty()
            })
            .expect("cooking trainer service");
        let spawn = &service.spawns[0];
        let destination = WorldPosition {
            map: spawn.map_id,
            point: Vec3::new(spawn.x, spawn.y, spawn.z),
            orientation: 0.0,
        };
        let position = WorldPosition {
            point: Vec3::new(
                destination.point.x + 10.0,
                destination.point.y,
                destination.point.z,
            ),
            ..destination
        };
        let player = EntityId(1);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(position);
        authoritative.inventory.money = CLASS_TRAINER_MONEY_RESERVE_COPPER + 1;
        authoritative.professions.known = true;
        authoritative.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                kind: wow_state::entities::EntityKind::Player,
                level: Some(10),
                health: Some((100, 100)),
                position: Some(position),
                ..Default::default()
            },
        );
        let mission = Mission::quest(MissionId(89));
        let intent = mission.intent.clone();
        let (mut engine, _) = test_engine(mission, authoritative);
        let snapshot = Snapshot::from_state(&engine.state.authoritative);

        assert!(engine.profession_trainer_travel_allowed(&snapshot));
        assert_eq!(engine.tick_profession_trainer_travel(&snapshot), Some(true));
        assert_eq!(engine.state.mission.intent, intent);
        assert!(engine.pending_movement.as_ref().is_some_and(|movement| {
            movement.purpose == MovementPurpose::ProfessionTrainer
                && movement.destination == destination
        }));
        engine.runtime_tuning.maintenance.auto_professions_enabled = false;
        assert!(!engine.profession_trainer_travel_allowed(&snapshot));
    }

    #[tokio::test]
    async fn bank_travel_queues_a_bounded_remembered_location_without_authorizing_bank_actions() {
        use wow_state::inventory::{InventoryItemInstance, ItemTemplateMetadata};

        let position = WorldPosition {
            map: 4,
            point: Vec3::new(10.0, 10.0, 5.0),
            orientation: 0.0,
        };
        let player = EntityId(1);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(position);
        authoritative.inventory.instances_authoritative = true;
        authoritative.inventory.free_slots = 2;
        authoritative.inventory.instances.insert(
            EntityId(90),
            InventoryItemInstance {
                item: 2589,
                guid: EntityId(90),
                backpack_slot: 23,
                count: 4,
            },
        );
        authoritative.inventory.item_metadata.insert(
            2589,
            ItemTemplateMetadata {
                item_class: 7,
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                kind: wow_state::entities::EntityKind::Player,
                health: Some((100, 100)),
                position: Some(position),
                ..Default::default()
            },
        );
        let mission = Mission::quest(MissionId(88));
        let intent = mission.intent.clone();
        let (mut engine, mut proxy) = test_engine(mission, authoritative);
        engine.state.mission.permissions = PermissionSet::MAINTENANCE | PermissionSet::MOVE;
        let destination = WorldPosition {
            point: Vec3::new(20.0, 10.0, 5.0),
            ..position
        };
        engine
            .remembered_bankers
            .insert(4, (destination, Instant::now()));
        let snapshot = Snapshot::from_state(&engine.state.authoritative);

        assert!(
            engine
                .tick_bank_deposit(&snapshot, Instant::now())
                .await
                .is_none()
        );
        assert_eq!(engine.tick_bank_travel(&snapshot), Some(true));
        assert_eq!(engine.state.mission.intent, intent);
        assert!(engine.pending_movement.as_ref().is_some_and(|movement| {
            movement.purpose == MovementPurpose::Banker && movement.destination == destination
        }));
        assert!(proxy.try_recv().is_err());

        engine.state.authoritative.inventory.free_slots = 3;
        engine.tick_mission().await;
        assert!(engine.pending_movement.is_none());
        let WorkerToProxy::Action(stop) = proxy.try_recv().expect("detour stop action") else {
            panic!("expected action");
        };
        assert_eq!(stop.command(), &GameplayCommand::StopMovement);
        assert_eq!(engine.state.mission.intent, intent);
    }

    #[tokio::test]
    async fn authoritative_player_level_initialization_and_increase_mark_training_due() {
        let player = EntityId(1);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        let (mut engine, _) = test_engine(Mission::quest(MissionId(1)), authoritative);
        let player_state = |level| wow_state::entities::EntityState {
            id: player,
            kind: wow_state::entities::EntityKind::Player,
            level: Some(level),
            health: Some((100, 100)),
            ..Default::default()
        };

        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::EntityUpsert {
                    entity: player_state(5),
                },
            ))
            .await;
        assert!(engine.class_training_due);
        engine.class_training_due = false;
        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::EntityUpsert {
                    entity: player_state(6),
                },
            ))
            .await;
        assert!(engine.class_training_due);
    }

    #[tokio::test]
    async fn trainer_memory_and_due_state_reset_when_character_or_session_changes() {
        let player = EntityId(1);
        let position = WorldPosition {
            map: 1,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 0.0,
        };
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        let (mut engine, _) = test_engine(Mission::quest(MissionId(1)), authoritative);
        engine.class_training_character = Some(player.0);
        engine.last_player_level = Some(10);
        engine.class_training_due = true;
        engine
            .remembered_class_trainers
            .insert(1, (position, Instant::now()));

        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::EnteredWorld {
                    character_guid: 2,
                    position: Some(position),
                },
            ))
            .await;
        assert_eq!(engine.class_training_character, Some(2));
        assert_eq!(engine.last_player_level, None);
        assert!(!engine.class_training_due);
        assert!(engine.remembered_class_trainers.is_empty());

        engine.class_training_due = true;
        engine
            .remembered_class_trainers
            .insert(1, (position, Instant::now()));
        engine
            .handle(LaneMessage::Observation(ProtocolObservation::LeftWorld))
            .await;
        assert_eq!(engine.class_training_character, None);
        assert_eq!(engine.last_player_level, None);
        assert!(!engine.class_training_due);
        assert!(engine.remembered_class_trainers.is_empty());
    }

    #[tokio::test]
    async fn banker_memory_uses_observed_service_position_and_resets_on_world_entry() {
        let player = EntityId(1);
        let position = WorldPosition {
            map: 4,
            point: Vec3::new(20.0, 30.0, 2.0),
            orientation: 0.0,
        };
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(position);
        let (mut engine, _) = test_engine(Mission::idle(), authoritative);
        let banker = EntityId(44);
        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::EntityUpsert {
                    entity: wow_state::entities::EntityState {
                        id: banker,
                        kind: wow_state::entities::EntityKind::Unit,
                        interactable: true,
                        npc_flags: Some(0x8),
                        position: Some(position),
                        ..Default::default()
                    },
                },
            ))
            .await;
        assert_eq!(
            engine.remembered_bankers.get(&4).map(|(p, _)| *p),
            Some(position)
        );

        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::EnteredWorld {
                    character_guid: player.0,
                    position: Some(position),
                },
            ))
            .await;
        assert!(engine.remembered_bankers.is_empty());
    }

    #[tokio::test]
    async fn nearby_banker_opens_authoritatively_then_receives_one_safe_deposit() {
        let player = EntityId(1);
        let banker = EntityId(2);
        let item_guid = EntityId(3);
        let position = WorldPosition {
            map: 1,
            point: Vec3::new(10.0, 10.0, 5.0),
            orientation: 0.0,
        };
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(position);
        authoritative.inventory.instances_authoritative = true;
        authoritative.inventory.free_slots = 2;
        authoritative.inventory.instances.insert(
            item_guid,
            wow_state::inventory::InventoryItemInstance {
                item: 2589,
                guid: item_guid,
                backpack_slot: 23,
                count: 4,
            },
        );
        authoritative.inventory.item_metadata.insert(
            2589,
            wow_state::inventory::ItemTemplateMetadata {
                item_class: 7,
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                kind: wow_state::entities::EntityKind::Player,
                health: Some((100, 100)),
                position: Some(position),
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            banker,
            wow_state::entities::EntityState {
                id: banker,
                kind: wow_state::entities::EntityKind::Unit,
                npc_flags: Some(0x8),
                interactable: true,
                position: Some(WorldPosition {
                    point: Vec3::new(11.0, 10.0, 5.0),
                    ..position
                }),
                ..Default::default()
            },
        );
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(1)), authoritative);
        let now = Instant::now();

        assert_eq!(
            engine
                .tick_bank_deposit(&Snapshot::from_state(&engine.state.authoritative), now)
                .await,
            Some(true)
        );
        let WorkerToProxy::Action(open) = proxy.try_recv().expect("bank-open action") else {
            panic!("expected bank-open action");
        };
        assert_eq!(open.command(), &GameplayCommand::BankActivate { banker });
        assert!(engine.bank_open_pending.is_some());

        assert_eq!(
            engine
                .tick_bank_deposit(
                    &Snapshot::from_state(&engine.state.authoritative),
                    now + Duration::from_secs(2),
                )
                .await,
            Some(true)
        );
        assert!(
            proxy.try_recv().is_err(),
            "do not repeat the bank-open action while waiting"
        );

        engine
            .handle(LaneMessage::Observation(ProtocolObservation::BankOpened {
                banker,
            }))
            .await;
        assert_eq!(
            engine
                .tick_bank_deposit(
                    &Snapshot::from_state(&engine.state.authoritative),
                    now + Duration::from_secs(3)
                )
                .await,
            Some(true)
        );
        let WorkerToProxy::Action(deposit) = proxy.try_recv().expect("bank-deposit action") else {
            panic!("expected bank-deposit action");
        };
        assert_eq!(
            deposit.command(),
            &GameplayCommand::BankDeposit {
                banker,
                item: 2589,
                item_guid,
                backpack_slot: 23,
            }
        );
        assert!(engine.bank_deposit_pending.is_some());
        assert_eq!(
            engine
                .tick_bank_deposit(
                    &Snapshot::from_state(&engine.state.authoritative),
                    now + Duration::from_secs(5),
                )
                .await,
            Some(true)
        );
        assert!(
            proxy.try_recv().is_err(),
            "do not repeat a deposit before inventory confirmation"
        );
    }

    #[tokio::test]
    async fn damaged_equipment_uses_a_live_repair_vendor() {
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let vendor = catalog
            .world()
            .vendor_services
            .iter()
            .find(|service| service.can_repair && !service.spawns.is_empty())
            .expect("catalog contains a repair vendor");
        let spawn = &vendor.spawns[0];
        let player_position = WorldPosition {
            map: spawn.map_id,
            point: Vec3::new(spawn.x, spawn.y, spawn.z),
            orientation: 0.0,
        };
        let player = EntityId(1);
        let repairer = EntityId(2);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(player_position);
        authoritative.inventory.equipment_condition = wow_state::inventory::EquipmentCondition {
            observed: true,
            lowest_durability_percent: Some(12),
            broken_items: 0,
        };
        authoritative.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                kind: wow_state::entities::EntityKind::Player,
                position: Some(player_position),
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            repairer,
            wow_state::entities::EntityState {
                id: repairer,
                kind: wow_state::entities::EntityKind::Unit,
                entry: vendor.entry_id,
                interactable: true,
                position: Some(WorldPosition {
                    point: Vec3::new(spawn.x + 1.0, spawn.y, spawn.z),
                    ..player_position
                }),
                ..Default::default()
            },
        );
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(1)), authoritative);

        assert!(
            engine
                .tick_equipment_repair(&Snapshot::from_state(&engine.state.authoritative))
                .await
                .unwrap()
        );
        assert_eq!(
            engine.last_dispatch,
            DispatchOutcome::Sent,
            "repair action did not dispatch: {:?}",
            engine.last_wait_reason
        );
        let WorkerToProxy::Action(action) = proxy.try_recv().expect("repair action") else {
            panic!("expected a repair action")
        };
        assert_eq!(
            action.command(),
            &GameplayCommand::RepairEquipment { vendor: repairer }
        );
        assert!(engine.repair_pending.is_some());
    }

    #[tokio::test]
    async fn equipment_repair_does_not_start_a_detour_for_an_active_group() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.inventory.equipment_condition = wow_state::inventory::EquipmentCondition {
            observed: true,
            lowest_durability_percent: Some(10),
            broken_items: 0,
        };
        authoritative.group.lifecycle = wow_state::group::GroupLifecycle::Active;
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(1)), authoritative);

        assert!(
            engine
                .tick_equipment_repair(&Snapshot::from_state(&engine.state.authoritative))
                .await
                .is_none()
        );
        assert!(proxy.try_recv().is_err());
    }

    #[test]
    fn repair_detour_stops_when_repair_is_complete_or_group_becomes_active() {
        let damaged = wow_state::inventory::EquipmentCondition {
            observed: true,
            lowest_durability_percent: Some(10),
            broken_items: 0,
        };
        let repaired = wow_state::inventory::EquipmentCondition {
            observed: true,
            lowest_durability_percent: Some(100),
            broken_items: 0,
        };
        assert!(repair_detour_should_continue(
            damaged,
            wow_state::group::GroupLifecycle::Solo
        ));
        assert!(!repair_detour_should_continue(
            repaired,
            wow_state::group::GroupLifecycle::Solo
        ));
        assert!(!repair_detour_should_continue(
            damaged,
            wow_state::group::GroupLifecycle::Active
        ));
    }

    #[test]
    fn quest_confirmation_wait_ends_at_the_shared_deadline() {
        let started = Instant::now();
        assert!(quest_confirmation_waiting(
            started,
            started + QUEST_CONFIRMATION_TIMEOUT - Duration::from_nanos(1)
        ));
        assert!(!quest_confirmation_waiting(
            started,
            started + QUEST_CONFIRMATION_TIMEOUT
        ));
    }

    #[tokio::test]
    async fn moved_mob_loot_uses_its_nearby_player_target_position() {
        let player = EntityId(1);
        let target = EntityId(2);
        let player_position = WorldPosition {
            map: 0,
            point: Vec3::new(-6455.0, 544.0, 387.0),
            orientation: 0.0,
        };
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(player_position);
        authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(-6477.6, 534.1, 387.5),
                    orientation: 0.0,
                }),
                health: Some((1, 100)),
                target: Some(player),
                ..Default::default()
            },
        );
        let (mut engine, _) = test_engine(Mission::quest(MissionId(1)), authoritative);

        assert!(
            engine
                .handle(LaneMessage::Observation(
                    ProtocolObservation::CreatureKilled {
                        killer: player,
                        victim: target,
                    }
                ))
                .await
        );

        let Some((queued_target, _, Some(corpse))) = &engine.post_combat_loot else {
            panic!("the owned kill must queue its corpse");
        };
        assert_eq!(*queued_target, target);
        assert_eq!(corpse.position, Some(player_position));
        assert_eq!(
            engine.state.authoritative.entities.0[&target].position,
            Some(player_position)
        );
    }

    #[test]
    fn later_corpse_updates_replace_the_cached_position_when_target_is_unknown() {
        let target = EntityId(2);
        let old_position = WorldPosition {
            map: 0,
            point: Vec3::new(10.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let current_position = WorldPosition {
            point: Vec3::new(20.0, 5.0, 1.0),
            ..old_position
        };
        let old = wow_state::entities::EntityState {
            id: target,
            position: Some(old_position),
            health: Some((0, 100)),
            ..Default::default()
        };
        let observed = wow_state::entities::EntityState {
            position: Some(current_position),
            ..old.clone()
        };
        let mut queued = Some((target, Instant::now(), Some(old)));

        let position = refresh_cached_post_combat_corpse(&mut queued, target, &observed);

        assert_eq!(queued.unwrap().2.unwrap().position, Some(current_position));
        assert_eq!(position, Some(current_position));
    }

    #[test]
    fn stale_corpse_update_keeps_a_corrected_position_for_the_same_attacker() {
        let target = EntityId(2);
        let attacker = EntityId(7);
        let death_site = WorldPosition {
            map: 0,
            point: Vec3::new(10.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let spawn = WorldPosition {
            point: Vec3::new(0.0, 0.0, 0.0),
            ..death_site
        };
        let old = wow_state::entities::EntityState {
            id: target,
            target: Some(attacker),
            position: Some(death_site),
            health: Some((0, 100)),
            ..Default::default()
        };
        let observed = wow_state::entities::EntityState {
            position: Some(spawn),
            ..old.clone()
        };
        let mut queued = Some((target, Instant::now(), Some(old)));

        let position = refresh_cached_post_combat_corpse(&mut queued, target, &observed);

        assert_eq!(queued.unwrap().2.unwrap().position, Some(death_site));
        assert_eq!(position, Some(death_site));
    }

    #[test]
    fn stale_corpse_update_keeps_a_corrected_position_after_target_is_cleared() {
        let target = EntityId(2);
        let attacker = EntityId(1);
        let spawn = WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let death_site = WorldPosition {
            point: Vec3::new(12.0, 0.0, 0.0),
            ..spawn
        };
        let old = wow_state::entities::EntityState {
            target: Some(attacker),
            position: Some(death_site),
            ..Default::default()
        };
        let observed = wow_state::entities::EntityState {
            target: None,
            position: Some(spawn),
            ..old.clone()
        };
        let mut queued = Some((target, Instant::now(), Some(old)));

        let position = refresh_cached_post_combat_corpse(&mut queued, target, &observed);

        assert_eq!(queued.unwrap().2.unwrap().position, Some(death_site));
        assert_eq!(position, Some(death_site));
    }

    #[test]
    fn corpse_approach_uses_its_latest_observed_position() {
        let player = EntityId(1);
        let target = EntityId(2);
        let mut state = wow_state::AuthoritativeState::default();
        state.session.character_guid = Some(player.0);
        state.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(10.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                health: Some((0, 100)),
                ..Default::default()
            },
        );
        let first = current_loot_approach(&Snapshot::from_state(&state), target)
            .expect("the first corpse position must have an approach");

        state.entities.0.get_mut(&target).unwrap().position = Some(WorldPosition {
            map: 0,
            point: Vec3::new(20.0, 5.0, 1.0),
            orientation: 0.0,
        });
        let current = current_loot_approach(&Snapshot::from_state(&state), target)
            .expect("the updated corpse position must have an approach");

        assert_ne!(first.0.point, current.0.point);
        assert_eq!(current.0.map, 0);
    }

    #[test]
    fn cached_post_combat_corpse_replaces_stale_world_position_for_loot_approach() {
        let target = EntityId(2);
        let stale_position = WorldPosition {
            map: 0,
            point: Vec3::new(-6508.82, 300.758, 370.446),
            orientation: 0.0,
        };
        let corpse_position = WorldPosition {
            map: 0,
            point: Vec3::new(-6499.8955, 326.72925, 368.62262),
            orientation: 1.84,
        };
        let corpse = wow_state::entities::EntityState {
            id: target,
            kind: wow_state::entities::EntityKind::Unit,
            position: Some(corpse_position),
            health: Some((0, 100)),
            ..Default::default()
        };
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::Unit,
                position: Some(stale_position),
                health: Some((0, 100)),
                ..Default::default()
            },
        );
        let (mut engine, _) = test_engine(Mission::quest(MissionId(1)), authoritative);
        engine.post_combat_loot = Some((target, Instant::now(), Some(corpse)));

        let actual_snapshot = engine.snapshot_with_post_combat_corpse(
            Snapshot::from_state(&engine.state.authoritative),
            target,
        );
        let mut expected_snapshot = Snapshot::from_state(&engine.state.authoritative);
        expected_snapshot
            .state
            .entities
            .0
            .get_mut(&target)
            .unwrap()
            .position = Some(corpse_position);

        assert_eq!(
            actual_snapshot.state.entities.0[&target].position,
            Some(corpse_position)
        );
        assert_eq!(
            current_loot_approach(&actual_snapshot, target),
            current_loot_approach(&expected_snapshot, target)
        );
    }

    #[tokio::test]
    async fn blocked_selected_target_queues_movement_before_retry() {
        let target = EntityId(2);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::Unit,
                hostile: true,
                health: Some((100, 100)),
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(10.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        let (mut engine, _) = test_engine(Mission::quest(MissionId(1)), authoritative);
        assert!(
            engine
                .handle(LaneMessage::Observation(ProtocolObservation::CastFailed {
                    spell: 686,
                    reason: 47,
                    target: Some(target),
                }))
                .await
        );

        assert!(
            engine
                .propose_command(
                    GameplayCommand::Cast {
                        spell: 686,
                        target: Some(target),
                    },
                    true,
                )
                .await
        );

        let movement = engine
            .pending_movement
            .as_ref()
            .expect("a line-of-sight failure must start recovery movement");
        assert_eq!(movement.destination.point, Vec3::new(0.0, 3.0, 0.0));
        assert_eq!(
            movement.resume,
            Some(GameplayCommand::Cast {
                spell: 686,
                target: Some(target),
            })
        );
    }

    #[test]
    fn corpse_not_targeting_player_keeps_its_world_position() {
        let player = EntityId(1);
        let mut state = wow_state::AuthoritativeState::default();
        state.session.character_guid = Some(player.0);
        state.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(20.0, 0.0, 0.0),
            orientation: 0.0,
        });
        let corpse_position = WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut corpse = wow_state::entities::EntityState {
            target: None,
            position: Some(corpse_position),
            ..Default::default()
        };

        assert!(!relocate_moved_mob_corpse(&state, &mut corpse));
        assert_eq!(corpse.position, Some(corpse_position));
    }

    #[test]
    fn owned_kill_without_target_evidence_keeps_observed_corpse_position() {
        let player = EntityId(1);
        let corpse_position = WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let player_position = WorldPosition {
            point: Vec3::new(20.0, 0.0, 0.0),
            ..corpse_position
        };
        let mut state = wow_state::AuthoritativeState::default();
        state.session.character_guid = Some(player.0);
        state.position.player = Some(player_position);
        let mut corpse = wow_state::entities::EntityState {
            target: None,
            position: Some(corpse_position),
            ..Default::default()
        };

        assert!(!relocate_moved_mob_corpse(&state, &mut corpse));
        assert_eq!(corpse.position, Some(corpse_position));
    }

    fn test_engine(
        mission: Mission,
        authoritative: wow_state::AuthoritativeState,
    ) -> (LaneEngine, mpsc::Receiver<WorkerToProxy>) {
        let (_lane_tx, lane_rx) = mpsc::channel(1);
        let (proxy_tx, proxy_rx) = mpsc::channel(4);
        let state = LaneState {
            lane: LaneId(1),
            worker: WorkerGeneration(1),
            ownership: OwnershipGeneration::ZERO,
            movement_epoch: MovementEpoch::ZERO,
            mission,
            mission_revision: MissionRevision::ZERO,
            permission_revision: PermissionRevision::ZERO,
            pause: PauseReasons::empty(),
            activation: ActivationStage::Act,
            authoritative,
            activity: ActivityArbiter::default(),
        };
        (LaneEngine::new(state, lane_rx, proxy_tx), proxy_rx)
    }

    fn mounted_combat_fixture() -> (LaneEngine, mpsc::Receiver<WorkerToProxy>) {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.entities.0.insert(
            EntityId(1),
            wow_state::entities::EntityState {
                id: EntityId(1),
                kind: wow_state::entities::EntityKind::Player,
                health: Some((100, 100)),
                mount_display_id: Some(123),
                unit_flags: Some(0),
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            EntityId(2),
            wow_state::entities::EntityState {
                id: EntityId(2),
                kind: wow_state::entities::EntityKind::Unit,
                hostile: true,
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        let mission = Mission {
            id: MissionId(91),
            intent: MissionIntent::Goal {
                text: "test dismount lifecycle".into(),
            },
            permissions: PermissionSet::MOVE | PermissionSet::COMBAT,
        };
        test_engine(mission, authoritative)
    }

    #[test]
    fn on_foot_gate_covers_combat_and_service_interactions() {
        let commands = [
            GameplayCommand::Attack(EntityId(1)),
            GameplayCommand::Cast {
                spell: 1,
                target: None,
            },
            GameplayCommand::AcceptQuest {
                quest: 1,
                giver: EntityId(1),
            },
            GameplayCommand::UseGameObject(EntityId(1)),
            GameplayCommand::Gather(EntityId(1)),
            GameplayCommand::VendorBuy {
                vendor: EntityId(1),
                item: 1,
                slot: 0,
                count: 1,
            },
            GameplayCommand::RepairEquipment {
                vendor: EntityId(1),
            },
            GameplayCommand::TrainerList {
                trainer: EntityId(1),
            },
            GameplayCommand::BankActivate {
                banker: EntityId(1),
            },
            GameplayCommand::AuctionBuy {
                query_generation: 1,
                listing_id: 1,
                max_buyout: 1,
            },
            GameplayCommand::TradeAccept {
                generation: 1,
                gift_only: true,
            },
            GameplayCommand::MailboxList {
                mailbox: EntityId(1),
            },
        ];
        assert!(commands.iter().all(LaneEngine::action_requires_on_foot));
        assert!(!LaneEngine::action_requires_on_foot(
            &GameplayCommand::UseItem {
                item: 1,
                target: None,
            }
        ));
        assert!(!LaneEngine::action_requires_on_foot(
            &GameplayCommand::MoveTo(Vec3::new(1.0, 0.0, 0.0),)
        ));
    }

    #[tokio::test]
    async fn mounted_on_foot_action_waits_for_authoritative_mount_and_form_removal() {
        let (mut engine, mut proxy) = mounted_combat_fixture();
        engine.state.authoritative.auras.by_entity.insert(
            EntityId(1),
            [(
                1,
                wow_state::auras::AuraInstance {
                    slot: 1,
                    spell: 783,
                    positive: Some(true),
                    caster: Some(EntityId(1)),
                    max_duration_ms: None,
                    remaining_ms: None,
                    observed_at_ms: None,
                },
            )]
            .into_iter()
            .collect(),
        );
        let action = ProposedAction {
            id: ActionId(44),
            task: TaskId(44),
            origin: PlanOrigin::SystemPolicy,
            stamp: engine.state.stamp(),
            command: GameplayCommand::Attack(EntityId(2)),
        };

        assert!(engine.submit(action).await);
        assert_eq!(engine.last_dispatch, DispatchOutcome::DeferredDismount);
        assert!(proxy.try_recv().is_err());

        assert!(engine.service_pending_dismount().await);
        let mut observed_cancel_mount = false;
        let mut observed_cancel_form = false;
        while let Ok(WorkerToProxy::Action(action)) = proxy.try_recv() {
            match action.command() {
                GameplayCommand::CancelMount => observed_cancel_mount = true,
                GameplayCommand::CancelAura { spell: 783 } => observed_cancel_form = true,
                command => panic!("unexpected cleanup command: {command:?}"),
            }
        }
        assert!(observed_cancel_mount);
        assert!(observed_cancel_form);

        engine
            .state
            .authoritative
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .mount_display_id = Some(0);
        engine
            .state
            .authoritative
            .auras
            .by_entity
            .remove(&EntityId(1));
        assert!(engine.service_pending_dismount().await);
        let Some(WorkerToProxy::Action(action)) = proxy.try_recv().ok() else {
            panic!("expected the held attack after authoritative cleanup")
        };
        assert_eq!(action.command(), &GameplayCommand::Attack(EntityId(2)));
        assert!(engine.pending_dismount.is_none());
    }

    #[tokio::test]
    async fn travel_cleanup_cancels_after_timeout_and_does_not_resume_stale_work() {
        let (mut engine, mut proxy) = mounted_combat_fixture();
        let action = ProposedAction {
            id: ActionId(45),
            task: TaskId(45),
            origin: PlanOrigin::SystemPolicy,
            stamp: engine.state.stamp(),
            command: GameplayCommand::Attack(EntityId(2)),
        };
        assert!(engine.submit(action).await);
        for attempt in 0..MAX_DISMOUNT_ATTEMPTS {
            if attempt > 0 {
                engine.pending_dismount.as_mut().unwrap().last_sent_at =
                    Some(Instant::now() - DISMOUNT_RETRY_INTERVAL);
            }
            assert!(engine.service_pending_dismount().await);
            while proxy.try_recv().is_ok() {}
        }
        assert_eq!(
            engine.pending_dismount.as_ref().unwrap().attempts,
            MAX_DISMOUNT_ATTEMPTS
        );
        assert!(engine.service_pending_dismount().await);
        assert!(engine.pending_dismount.is_none());
        assert!(proxy.try_recv().is_err());

        let action = ProposedAction {
            id: ActionId(46),
            task: TaskId(46),
            origin: PlanOrigin::SystemPolicy,
            stamp: engine.state.stamp(),
            command: GameplayCommand::Attack(EntityId(2)),
        };
        assert!(engine.submit(action).await);
        engine.state.mission_revision = MissionRevision(1);
        assert!(engine.service_pending_dismount().await);
        assert!(engine.pending_dismount.is_none());
        assert!(proxy.try_recv().is_err());

        engine.begin_travel_cleanup();
        engine.state.mission_revision = MissionRevision(2);
        assert!(engine.service_pending_dismount().await);
        assert!(engine.pending_dismount.is_none());
        assert!(proxy.try_recv().is_err());
    }

    #[tokio::test]
    async fn quest_turn_in_selects_the_best_authoritative_equipment_reward() {
        use wow_state::{
            entities::{EntityKind, EntityState},
            inventory::ItemTemplateMetadata,
            quests::{QuestTurnInDialog, QuestTurnInStage},
        };

        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.capabilities.class_id = Some(1);
        authoritative.capabilities.specialization_tree = Some(0);
        authoritative.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                level: Some(60),
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            EntityId(77),
            EntityState {
                id: EntityId(77),
                kind: EntityKind::Unit,
                interactable: true,
                ..Default::default()
            },
        );
        authoritative.quests.active.insert(
            55,
            wow_state::quests::QuestProgress {
                complete: true,
                objectives: Vec::new(),
            },
        );
        authoritative.inventory.equipped_items.insert(0, 100);
        let item = |item_level, strength| ItemTemplateMetadata {
            item_class: 4,
            subclass: 1,
            quality: 2,
            sell_price: item_level,
            inventory_type: 1,
            allowable_class: u32::MAX,
            item_level,
            required_level: 1,
            stats: vec![(4, strength)],
            armor: item_level,
            ..Default::default()
        };
        authoritative
            .inventory
            .item_metadata
            .insert(100, item(10, 1));
        authoritative
            .inventory
            .item_metadata
            .insert(200, item(18, 10));
        authoritative
            .inventory
            .item_metadata
            .insert(201, item(20, 15));
        authoritative.quests.turn_in.insert(
            55,
            QuestTurnInDialog {
                giver: EntityId(77),
                stage: QuestTurnInStage::OfferReward {
                    reward_items: vec![200, 201],
                },
            },
        );
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(1)), authoritative);

        assert!(engine.tick_complete_quest(55).await);

        let WorkerToProxy::Action(action) = proxy.try_recv().expect("reward selection action")
        else {
            panic!("expected quest reward action")
        };
        assert_eq!(
            action.command(),
            &GameplayCommand::ChooseQuestReward {
                quest: 55,
                giver: EntityId(77),
                reward: 1,
            }
        );
    }

    #[tokio::test]
    async fn quest_reward_waits_for_metadata_then_uses_a_bounded_fallback() {
        use wow_state::{
            entities::{EntityKind, EntityState},
            quests::{QuestTurnInDialog, QuestTurnInStage},
        };

        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                level: Some(60),
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            EntityId(77),
            EntityState {
                id: EntityId(77),
                kind: EntityKind::Unit,
                interactable: true,
                ..Default::default()
            },
        );
        authoritative.quests.active.insert(
            55,
            wow_state::quests::QuestProgress {
                complete: true,
                objectives: Vec::new(),
            },
        );
        authoritative.quests.turn_in.insert(
            55,
            QuestTurnInDialog {
                giver: EntityId(77),
                stage: QuestTurnInStage::OfferReward {
                    reward_items: vec![200, 201],
                },
            },
        );
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(1)), authoritative);

        assert!(engine.tick_complete_quest(55).await);
        let WorkerToProxy::Action(query) = proxy.try_recv().expect("reward item query") else {
            panic!("expected reward metadata query")
        };
        assert_eq!(query.command(), &GameplayCommand::QueryItem { item: 200 });

        engine.reward_metadata_waiting = Some((
            55,
            Instant::now() - wow_policy::questing::rewards::metadata_wait(),
        ));
        assert!(engine.tick_complete_quest(55).await);
        let WorkerToProxy::Action(choice) = proxy.try_recv().expect("reward fallback") else {
            panic!("expected quest reward selection")
        };
        assert_eq!(
            choice.command(),
            &GameplayCommand::ChooseQuestReward {
                quest: 55,
                giver: EntityId(77),
                reward: 0,
            }
        );
    }

    #[tokio::test]
    async fn full_bag_vendor_relief_travels_to_a_known_sell_vendor() {
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let initial_spawn = catalog
            .world()
            .vendor_services
            .iter()
            .find(|vendor| vendor.can_sell && !vendor.spawns.is_empty())
            .and_then(|vendor| vendor.spawns.first().map(|spawn| (vendor.entry_id, spawn)))
            .expect("the embedded catalog contains a sell vendor spawn");
        let origin = Vec3::new(
            initial_spawn.1.x + 50.0,
            initial_spawn.1.y + 50.0,
            initial_spawn.1.z,
        );
        let (_, destination) = catalog
            .nearest_vendor(
                wow_infra::world_knowledge::VendorKind::Sell,
                initial_spawn.1.map_id,
                origin,
            )
            .expect("the catalog contains a sell vendor on this map");
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.position.player = Some(WorldPosition {
            map: initial_spawn.1.map_id,
            point: origin,
            orientation: 0.0,
        });
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(1)), authoritative);
        engine.bag_full_loot_targets.insert(EntityId(702));

        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        assert_eq!(engine.tick_bag_relief(&snapshot).await, Some(true));
        assert!(proxy.try_recv().is_err());
        let movement = engine
            .pending_movement
            .as_ref()
            .expect("full-bag relief should queue travel to a vendor");
        assert_eq!(movement.destination.point, destination);
        assert_eq!(movement.destination.map, initial_spawn.1.map_id);
    }

    #[test]
    fn movement_step_uses_elapsed_packet_time_with_a_bounded_gap() {
        let now = Instant::now();
        assert!((movement_step_distance(7.0, None, now) - 0.7).abs() < 0.001);
        assert!(
            (movement_step_distance(7.0, Some(now - Duration::from_millis(200)), now) - 1.4).abs()
                < 0.001
        );
        assert!(
            (movement_step_distance(7.0, Some(now - Duration::from_secs(2)), now) - 1.75).abs()
                < 0.001
        );
    }

    #[test]
    fn movement_progress_ignores_replayed_positions_and_requires_authoritative_displacement() {
        let (mut engine, _proxy_rx) = test_engine(
            Mission::quest(MissionId(1)),
            wow_state::AuthoritativeState::default(),
        );
        let start = Vec3::new(1.0, 2.0, 3.0);
        engine.queue_movement(
            Vec3::new(10.0, 2.0, 3.0),
            1.0,
            None,
            None,
            PlanOrigin::SystemPolicy,
            QuestWorkRuntime {
                id: QuestWorkId(7),
                key: QuestWorkKey::AcquireQuest { quest: None },
            },
            MovementPurpose::SearchArea,
        );
        let movement = engine.pending_movement.as_mut().expect("queued movement");
        movement.last_progress_position = Some(start);
        movement.last_player_client_time = Some(40);
        movement.last_progress_at = Instant::now() - Duration::from_secs(6);
        let stalled_since = movement.last_progress_at;

        engine.observe_movement_position(
            "player",
            WorldPosition {
                map: 0,
                point: start,
                orientation: 0.0,
            },
            Some(40),
            StateRevision(1),
        );
        assert_eq!(
            engine.pending_movement.as_ref().unwrap().last_progress_at,
            stalled_since
        );

        engine.observe_movement_position(
            "player",
            WorldPosition {
                map: 0,
                point: start,
                orientation: 0.0,
            },
            Some(41),
            StateRevision(2),
        );
        assert_eq!(
            engine.pending_movement.as_ref().unwrap().last_progress_at,
            stalled_since
        );
        assert_eq!(
            engine.pending_movement.as_ref().unwrap().progress_source,
            Some("player")
        );
        assert_eq!(
            engine.pending_movement.as_ref().unwrap().progress_revision,
            Some(StateRevision(2))
        );

        engine.observe_movement_position(
            "player",
            WorldPosition {
                map: 0,
                point: Vec3::new(2.0, 2.0, 3.0),
                orientation: 0.0,
            },
            Some(42),
            StateRevision(3),
        );
        assert!(engine.pending_movement.as_ref().unwrap().last_progress_at > stalled_since);
    }

    #[tokio::test]
    async fn visible_quest_target_does_not_cancel_search_area_movement() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        authoritative.quests.active.insert(
            218,
            wow_state::quests::QuestProgress {
                complete: false,
                objectives: vec![0],
            },
        );
        authoritative.quests.definitions.insert(
            218,
            wow_state::quests::QuestDefinition {
                quest: 218,
                title: "Test cave objective".into(),
                poi_map: Some(0),
                poi_x: Some(100.0),
                poi_y: Some(100.0),
                targets: vec![wow_state::quests::QuestTargetObjective {
                    slot: 0,
                    kind: wow_state::quests::QuestTargetKind::Creature,
                    entry: 7,
                    required: 1,
                    item_drop: 0,
                    text: "Test target".into(),
                }],
                items: vec![],
            },
        );
        authoritative.entities.0.insert(
            EntityId(9),
            wow_state::entities::EntityState {
                id: EntityId(9),
                entry: 7,
                kind: wow_state::entities::EntityKind::Unit,
                hostile: true,
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(20.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        let (mut engine, _proxy_rx) = test_engine(Mission::quest(MissionId(1)), authoritative);
        engine.queue_movement(
            Vec3::new(100.0, 100.0, 0.0),
            QUEST_SEARCH_ARRIVAL_RANGE,
            None,
            None,
            PlanOrigin::Deterministic,
            QuestWorkRuntime {
                id: QuestWorkId(7),
                key: QuestWorkKey::TravelToObjective {
                    quest: 218,
                    objective: 0,
                    destination: WorldPosition {
                        map: 0,
                        point: Vec3::new(100.0, 100.0, 0.0),
                        orientation: 0.0,
                    },
                },
            },
            MovementPurpose::SearchArea,
        );

        assert!(engine.tick_movement().await);
        let movement = engine
            .pending_movement
            .as_ref()
            .expect("visible target must not cancel the cave search route");
        assert_eq!(movement.purpose, MovementPurpose::SearchArea);
        assert_eq!(movement.destination.point, Vec3::new(100.0, 100.0, 0.0));
    }

    fn combat_engine() -> (LaneEngine, mpsc::Receiver<WorkerToProxy>) {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.position.player = Some(wow_domain::WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        authoritative.capabilities.class_id = Some(1);
        authoritative.capabilities.specialization_tree = Some(0);
        authoritative.entities.0.insert(
            EntityId(1),
            wow_state::entities::EntityState {
                id: EntityId(1),
                kind: wow_state::entities::EntityKind::Player,
                position: authoritative.position.player,
                power_type: Some(1),
                power: Some((20, 100)),
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            EntityId(9),
            wow_state::entities::EntityState {
                id: EntityId(9),
                kind: wow_state::entities::EntityKind::Unit,
                hostile: true,
                position: Some(wow_domain::WorldPosition {
                    map: 0,
                    point: Vec3::new(2.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        test_engine(Mission::quest(MissionId(9)), authoritative)
    }

    #[tokio::test]
    async fn rejected_post_combat_loot_keeps_corpse_queued_until_retry_then_loots_it() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        let target = EntityId(77);
        authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::Unit,
                health: Some((0, 100)),
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(2.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        let (mut engine, mut proxy_rx) = test_engine(Mission::quest(MissionId(1)), authoritative);
        engine.post_combat_loot = Some((target, Instant::now(), None));
        engine.pending_quest_action = Some(PendingQuestAction::CorpseLoot {
            target,
            baseline_generation: 0,
            started: Instant::now(),
        });

        assert!(
            engine
                .handle(LaneMessage::Observation(
                    ProtocolObservation::LootRejected {
                        target,
                        loot_type: 0,
                        error: None,
                    }
                ))
                .await
        );
        assert!(engine.pending_quest_action.is_none());
        assert_eq!(
            engine
                .post_combat_loot
                .as_ref()
                .map(|(queued, _, _)| *queued),
            Some(target)
        );

        assert!(engine.tick_quest().await);
        assert!(proxy_rx.try_recv().is_err());

        engine.loot_retry_after.get_mut(&target).unwrap().1 = Some(Instant::now());
        assert!(engine.tick_quest().await);
        let Some(PendingQuestAction::CorpseLootDelay { arrived_at, .. }) =
            engine.pending_quest_action.as_mut()
        else {
            panic!("an in-range corpse must continue through the arrival delay");
        };
        *arrived_at = Instant::now() - Duration::from_secs(1);
        assert!(engine.tick_quest().await);
        let Ok(WorkerToProxy::Action(stop)) = proxy_rx.try_recv() else {
            panic!("corpse arrival must stop movement before loot");
        };
        assert_eq!(stop.command(), &GameplayCommand::StopMovement);
        let Ok(WorkerToProxy::Action(loot)) = proxy_rx.try_recv() else {
            panic!("the retained post-combat corpse must receive a loot request");
        };
        assert_eq!(loot.command(), &GameplayCommand::Loot(target));
    }

    #[tokio::test]
    async fn full_bag_rejection_blocks_retry_until_space_is_available() {
        let (mut engine, _proxy_rx) = test_engine(
            Mission::quest(MissionId(1)),
            wow_state::AuthoritativeState::default(),
        );
        let target = EntityId(77);
        engine.state.authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::Unit,
                health: Some((0, 100)),
                ..Default::default()
            },
        );
        engine.pending_quest_action = Some(PendingQuestAction::CorpseLoot {
            target,
            baseline_generation: 0,
            started: Instant::now(),
        });
        engine.post_combat_loot = Some((target, Instant::now(), None));

        assert!(
            engine
                .handle(LaneMessage::Observation(
                    ProtocolObservation::LootRejected {
                        target,
                        loot_type: 0,
                        error: Some(12),
                    }
                ))
                .await
        );
        assert_eq!(engine.loot_retry_after[&target], (u8::MAX, None));
        assert!(engine.bag_full_loot_targets.contains(&target));
        assert!(engine.pending_quest_action.is_none());
        assert!(engine.post_combat_loot.is_none());

        assert!(
            engine
                .handle(LaneMessage::Observation(
                    ProtocolObservation::InventoryFreeSlots { count: 0 }
                ))
                .await
        );
        assert!(engine.bag_full_loot_targets.contains(&target));

        assert!(
            engine
                .handle(LaneMessage::Observation(
                    ProtocolObservation::InventoryFreeSlots { count: 1 }
                ))
                .await
        );
        assert!(!engine.bag_full_loot_targets.contains(&target));
        assert!(!engine.loot_retry_after.contains_key(&target));

        assert!(!engine.looted_corpses.contains(&target));
    }

    #[test]
    fn quest_loot_does_not_count_a_disappeared_corpse_as_item_progress() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.inventory.items.insert(750, 2);
        let (mut engine, _proxy_rx) = test_engine(Mission::quest(MissionId(1)), authoritative);
        let target = EntityId(78);
        engine.pending_quest_action = Some(PendingQuestAction::Loot {
            item: 750,
            target,
            baseline_count: 2,
            baseline_generation: 0,
            started: Instant::now(),
        });

        assert!(!engine.pending_quest_action_blocks());
        assert!(engine.pending_quest_action.is_none());
        assert!(!engine.looted_corpses.contains(&target));
    }

    #[test]
    fn quest_loot_completes_only_after_authoritative_item_count_increases() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.inventory.items.insert(750, 3);
        let (mut engine, _proxy_rx) = test_engine(Mission::quest(MissionId(1)), authoritative);
        let target = EntityId(79);
        engine.pending_quest_action = Some(PendingQuestAction::Loot {
            item: 750,
            target,
            baseline_count: 2,
            baseline_generation: 0,
            started: Instant::now(),
        });

        assert!(!engine.pending_quest_action_blocks());
        assert!(engine.pending_quest_action.is_none());
        assert!(engine.looted_corpses.contains(&target));
    }

    #[test]
    fn one_loot_close_without_an_open_does_not_complete_corpse_loot() {
        let (mut engine, _proxy_rx) = test_engine(
            Mission::quest(MissionId(1)),
            wow_state::AuthoritativeState::default(),
        );
        let target = EntityId(77);
        engine.state.authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::Unit,
                health: Some((0, 100)),
                ..Default::default()
            },
        );
        engine.pending_quest_action = Some(PendingQuestAction::CorpseLoot {
            target,
            baseline_generation: 0,
            started: Instant::now(),
        });
        reduce(
            &mut engine.state.authoritative,
            ProtocolObservation::LootClosed {
                ownership: wow_state::LootOwnership::Bot,
            },
        );

        assert!(engine.pending_quest_action_blocks());
        assert!(!engine.looted_corpses.contains(&target));
    }

    #[tokio::test]
    async fn corpse_loot_delay_starts_only_after_authoritative_in_range_arrival() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        let target = EntityId(77);
        authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::Unit,
                health: Some((0, 100)),
                position: None,
                ..Default::default()
            },
        );
        let (mut engine, mut proxy_rx) = test_engine(Mission::quest(MissionId(1)), authoritative);
        let dispatch = || PendingQuestAction::CorpseLoot {
            target,
            baseline_generation: 0,
            started: Instant::now(),
        };

        engine
            .dispatch_quest_semantic(GameplayCommand::Loot(target), dispatch())
            .await;
        assert!(engine.pending_quest_action.is_none());
        assert!(proxy_rx.try_recv().is_err());

        engine
            .state
            .authoritative
            .entities
            .0
            .get_mut(&target)
            .unwrap()
            .position = Some(WorldPosition {
            map: 0,
            point: Vec3::new(2.0, 0.0, 0.0),
            orientation: 0.0,
        });
        engine
            .dispatch_quest_semantic(GameplayCommand::Loot(target), dispatch())
            .await;
        let Some(PendingQuestAction::CorpseLootDelay { arrived_at, .. }) =
            engine.pending_quest_action.as_mut()
        else {
            panic!("in-range corpse must enter the arrival delay");
        };
        let Ok(WorkerToProxy::Action(stop)) = proxy_rx.try_recv() else {
            panic!("corpse arrival must send a stop movement action");
        };
        assert_eq!(stop.command(), &GameplayCommand::StopMovement);
        *arrived_at = Instant::now() - Duration::from_secs(1);
        assert!(!engine.pending_quest_action_blocks());
        assert!(engine.corpses_ready_to_loot.contains(&target));
        engine.state.authoritative.position.moving = true;
        engine
            .dispatch_quest_semantic(GameplayCommand::Loot(target), dispatch())
            .await;
        let Ok(WorkerToProxy::Action(stop)) = proxy_rx.try_recv() else {
            panic!("moving character must receive another stop action");
        };
        assert_eq!(stop.command(), &GameplayCommand::StopMovement);
        let Some(PendingQuestAction::CorpseLootDelay { arrived_at, .. }) =
            engine.pending_quest_action.as_mut()
        else {
            panic!("looting must wait until movement stops");
        };
        engine.state.authoritative.position.moving = false;
        *arrived_at = Instant::now() - Duration::from_secs(1);
        assert!(!engine.pending_quest_action_blocks());

        engine
            .dispatch_quest_semantic(GameplayCommand::Loot(target), dispatch())
            .await;
        let Ok(WorkerToProxy::Action(loot)) = proxy_rx.try_recv() else {
            panic!("stopped character must send the loot action");
        };
        assert_eq!(loot.command(), &GameplayCommand::Loot(target));
        assert!(matches!(
            engine.pending_quest_action,
            Some(PendingQuestAction::CorpseLoot { target: pending, .. }) if pending == target
        ));
    }

    #[test]
    fn opened_and_closed_loot_marks_corpse_complete() {
        let (mut engine, _proxy_rx) = test_engine(
            Mission::quest(MissionId(1)),
            wow_state::AuthoritativeState::default(),
        );
        let target = EntityId(77);
        engine.pending_quest_action = Some(PendingQuestAction::CorpseLoot {
            target,
            baseline_generation: 0,
            started: Instant::now(),
        });
        reduce(
            &mut engine.state.authoritative,
            ProtocolObservation::LootOpened {
                target,
                ownership: wow_state::LootOwnership::Bot,
            },
        );
        reduce(
            &mut engine.state.authoritative,
            ProtocolObservation::LootClosed {
                ownership: wow_state::LootOwnership::Bot,
            },
        );

        assert!(!engine.pending_quest_action_blocks());
        assert!(engine.looted_corpses.contains(&target));
    }

    #[tokio::test]
    async fn player_opened_loot_cancels_matching_bot_loot_intents_without_crediting_success() {
        let (mut engine, _proxy_rx) = test_engine(
            Mission::quest(MissionId(1)),
            wow_state::AuthoritativeState::default(),
        );
        let target = EntityId(78);
        engine.pending_quest_action = Some(PendingQuestAction::CorpseLoot {
            target,
            baseline_generation: 0,
            started: Instant::now(),
        });
        engine.post_combat_loot = Some((target, Instant::now(), None));

        assert!(
            engine
                .handle(LaneMessage::Observation(ProtocolObservation::LootOpened {
                    target,
                    ownership: wow_state::LootOwnership::Player,
                }))
                .await
        );

        assert!(engine.pending_quest_action.is_none());
        assert!(engine.post_combat_loot.is_none());
        assert!(!engine.looted_corpses.contains(&target));
    }

    #[tokio::test]
    async fn quest_scheduler_searches_static_starters_for_empty_and_active_logs() {
        let start = [0, 1, 530, 571]
            .into_iter()
            .find_map(|map| {
                wow_policy::questing::static_hints::nearby_quest_starts(
                    map,
                    Vec3::default(),
                    Some(1),
                    f32::MAX,
                )
                .into_iter()
                .next()
                .map(|start| (map, start.location))
            })
            .expect("the embedded AzerothCore catalog contains quest starters");

        for active_quest in [false, true] {
            let (map, location) = start;
            let mut authoritative = wow_state::AuthoritativeState::default();
            authoritative.session.in_world = true;
            authoritative.capabilities.class_id = Some(1);
            authoritative.position.player = Some(wow_domain::WorldPosition {
                map,
                point: location,
                orientation: 0.0,
            });
            if active_quest {
                authoritative
                    .quests
                    .active
                    .insert(7, wow_state::quests::QuestProgress::default());
            }
            let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(17)), authoritative);

            assert!(engine.tick_quest().await);
            let movement = engine
                .pending_movement
                .as_ref()
                .expect("the quest scheduler should search the nearby static starter");
            assert_eq!(movement.purpose, MovementPurpose::SearchArea);
            assert_eq!(movement.destination.point, location);
            assert_eq!(movement.resume, Some(GameplayCommand::QueryQuestGivers));
            assert!(matches!(
                &movement.work.key,
                QuestWorkKey::AcquireQuest { quest: None }
            ));
            assert!(proxy.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn quest_scheduler_queries_server_when_no_static_starters_are_nearby() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.capabilities.class_id = Some(1);
        authoritative.position.player = Some(wow_domain::WorldPosition {
            map: u32::MAX,
            point: Vec3::default(),
            orientation: 0.0,
        });
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(18)), authoritative);

        assert!(engine.tick_quest().await);
        let WorkerToProxy::Action(action) = proxy
            .try_recv()
            .expect("the scheduler should request authoritative giver statuses")
        else {
            panic!("expected a quest-giver query action")
        };
        assert_eq!(action.command(), &GameplayCommand::QueryQuestGivers);
    }

    #[tokio::test]
    async fn quest_scheduler_skips_offers_for_active_or_completed_quests() {
        let active_giver = EntityId(8);
        let completed_giver = EntityId(9);
        let available_giver = EntityId(10);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative
            .quests
            .active
            .insert(218, wow_state::quests::QuestProgress::default());
        authoritative.quests.completed.push(219);
        authoritative.quests.offers.extend([
            (
                218,
                wow_state::quests::QuestOffer {
                    giver: active_giver,
                    icon: 0,
                },
            ),
            (
                219,
                wow_state::quests::QuestOffer {
                    giver: completed_giver,
                    icon: 0,
                },
            ),
            (
                220,
                wow_state::quests::QuestOffer {
                    giver: available_giver,
                    icon: 0,
                },
            ),
        ]);
        for giver in [active_giver, completed_giver, available_giver] {
            authoritative.entities.0.insert(
                giver,
                wow_state::entities::EntityState {
                    id: giver,
                    ..Default::default()
                },
            );
        }
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(19)), authoritative);

        assert!(engine.tick_quest().await);
        let WorkerToProxy::Action(action) = proxy.try_recv().expect("valid offer should be sent")
        else {
            panic!("expected a quest acceptance action")
        };
        assert_eq!(
            action.command(),
            &GameplayCommand::AcceptQuest {
                quest: 220,
                giver: available_giver,
            }
        );
        assert_eq!(engine.pending_accept.map(|(quest, _, _)| quest), Some(220));
    }

    #[tokio::test]
    async fn targeted_action_waits_for_authoritative_facing_update() {
        let target = EntityId(9);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.position.player = Some(wow_domain::WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        authoritative.entities.0.insert(
            target,
            wow_state::entities::EntityState {
                id: target,
                kind: wow_state::entities::EntityKind::GameObject,
                interactable: true,
                position: Some(wow_domain::WorldPosition {
                    map: 0,
                    point: Vec3::new(0.0, 2.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(9)), authoritative);

        assert!(
            engine
                .propose_command(GameplayCommand::UseGameObject(target), true)
                .await
        );
        assert_eq!(engine.last_dispatch, DispatchOutcome::DeferredSpatial);
        let WorkerToProxy::Action(facing) =
            proxy.try_recv().expect("facing action should be queued")
        else {
            panic!("expected facing action")
        };
        assert!(matches!(
            facing.command(),
            GameplayCommand::FaceDirection { orientation }
                if (*orientation - std::f32::consts::FRAC_PI_2).abs() < 1.0e-5
        ));

        assert!(
            engine
                .propose_command(GameplayCommand::UseGameObject(target), true)
                .await
        );
        assert_eq!(engine.last_dispatch, DispatchOutcome::DeferredSpatial);
        assert!(
            proxy.try_recv().is_err(),
            "interaction must wait for server facing confirmation"
        );

        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::PlayerPosition {
                    position: wow_domain::WorldPosition {
                        map: 0,
                        point: Vec3::new(0.0, 0.0, 0.0),
                        orientation: std::f32::consts::FRAC_PI_2,
                    },
                    moving: false,
                    flags: 0,
                    client_time: 0,
                },
            ))
            .await;
        assert!(
            engine
                .propose_command(GameplayCommand::UseGameObject(target), true)
                .await
        );
        let WorkerToProxy::Action(interaction) = proxy
            .try_recv()
            .expect("interaction should be queued after facing confirmation")
        else {
            panic!("expected interaction action after facing confirmation")
        };
        assert_eq!(
            interaction.command(),
            &GameplayCommand::UseGameObject(target)
        );
    }

    #[tokio::test]
    async fn quest_and_survival_combat_share_selection_and_action_validation() {
        let target = EntityId(9);
        let (mut quest_engine, mut quest_proxy) = combat_engine();
        assert!(quest_engine.dispatch_combat_target(target, false).await);
        let WorkerToProxy::Action(quest_action) = quest_proxy.recv().await.unwrap() else {
            panic!("expected validated quest combat action")
        };
        assert_eq!(quest_action.command(), &GameplayCommand::Attack(target));
        assert_eq!(quest_action.origin(), PlanOrigin::SystemPolicy);
        assert!(matches!(
            quest_engine.pending_quest_action,
            Some(PendingQuestAction::Combat { target: pending, .. }) if pending == target
        ));

        let (mut survival_engine, mut survival_proxy) = combat_engine();
        assert!(survival_engine.dispatch_combat_target(target, true).await);
        let WorkerToProxy::Action(survival_action) = survival_proxy.recv().await.unwrap() else {
            panic!("expected validated survival combat action")
        };
        assert_eq!(survival_action.command(), quest_action.command());
        assert_eq!(survival_action.origin(), PlanOrigin::Recovery);
    }

    #[tokio::test]
    async fn emergency_non_potion_health_items_ignore_potion_lockout() {
        let target = EntityId(9);
        let (mut engine, _) = combat_engine();
        engine
            .state
            .authoritative
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .health = Some((15, 100));
        engine.state.authoritative.inventory.instances.insert(
            EntityId(11),
            wow_state::inventory::InventoryItemInstance {
                item: 101,
                guid: EntityId(11),
                backpack_slot: 23,
                count: 1,
            },
        );
        engine.state.authoritative.inventory.item_metadata.insert(
            101,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Whipper Root Tuber".into(),
                item_class: 0,
                use_spell_id: 900,
                ..Default::default()
            },
        );
        engine.combat_potion_lockout_until = Some(Instant::now() + COMBAT_POTION_FALLBACK_LOCKOUT);

        let snapshot = Snapshot::from_state(&engine.state.authoritative);
        let command = engine
            .select_combat_emergency_health_item(&snapshot, target, true)
            .expect("non-potion survival items must ignore Potion Sickness fallback lockout");
        assert_eq!(
            command,
            GameplayCommand::UseItemInstance {
                item: 101,
                item_guid: EntityId(11),
                backpack_slot: 23,
                spell: 900,
                target: Some(EntityId(1)),
                cast_count: 0,
            }
        );
    }

    #[tokio::test]
    async fn gather_and_grind_dispatch_only_matching_authorized_observations() {
        let mut state = wow_state::AuthoritativeState::default();
        state.session.in_world = true;
        state.entities.0.insert(
            EntityId(21),
            wow_state::entities::EntityState {
                id: EntityId(21),
                kind: wow_state::entities::EntityKind::GameObject,
                name: Some("Copper Vein".into()),
                interactable: true,
                ..Default::default()
            },
        );
        let (mut engine, mut rx) = test_engine(
            Mission::gather(MissionId(1), " copper vein "),
            state.clone(),
        );
        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = rx.recv().await.unwrap() else {
            panic!("expected gather action")
        };
        assert_eq!(action.command(), &GameplayCommand::Gather(EntityId(21)));

        state.entities.0.insert(
            EntityId(22),
            wow_state::entities::EntityState {
                id: EntityId(22),
                kind: wow_state::entities::EntityKind::Unit,
                name: Some("Wolf".into()),
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(2.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                hostile: true,
                health: Some((10, 10)),
                ..Default::default()
            },
        );
        state.session.character_guid = Some(1);
        state.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.entities.0.insert(
            EntityId(1),
            wow_state::entities::EntityState {
                id: EntityId(1),
                kind: wow_state::entities::EntityKind::Player,
                health: Some((100, 100)),
                level: Some(10),
                ..Default::default()
            },
        );
        let (mut engine, mut rx) = test_engine(Mission::grind(MissionId(2), "wolf"), state);
        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = rx.recv().await.unwrap() else {
            panic!("expected grind action")
        };
        assert_eq!(action.command(), &GameplayCommand::Attack(EntityId(22)));
    }

    #[tokio::test]
    async fn gather_mission_searches_trusted_local_spawns_until_a_live_node_appears() {
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let node = catalog
            .world()
            .gather_nodes
            .iter()
            .find(|node| !node.spawns.is_empty())
            .expect("embedded catalog contains gathering spawns");
        let spawn = &node.spawns[0];
        let skill = match node.kind {
            wow_infra::world_knowledge::GatheringKind::Mining => 186,
            wow_infra::world_knowledge::GatheringKind::Herbalism => 182,
            wow_infra::world_knowledge::GatheringKind::Fishing => 356,
        };
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.position.player = Some(WorldPosition {
            map: spawn.map_id,
            point: Vec3::new(spawn.x + 20.0, spawn.y, spawn.z),
            orientation: 0.0,
        });
        authoritative.professions.known = true;
        authoritative
            .professions
            .skills
            .insert(skill, (u16::MAX, u16::MAX));
        let (mut engine, mut proxy) = test_engine(
            Mission::gather(MissionId(1), node.name.clone()),
            authoritative,
        );

        assert!(engine.tick_gather(&node.name).await);

        assert!(
            proxy.try_recv().is_err(),
            "a static hint cannot authorize gathering"
        );
        let movement = engine
            .pending_movement
            .as_ref()
            .expect("a trusted spawn hint should create search movement");
        assert_eq!(movement.destination.map, spawn.map_id);
        assert_eq!(
            movement.destination.point,
            Vec3::new(spawn.x, spawn.y, spawn.z)
        );
    }

    #[tokio::test]
    async fn gather_authority_and_fishing_capability_gates_prevent_actions() {
        let mut state = wow_state::AuthoritativeState::default();
        state.session.in_world = true;
        let (mut engine, mut rx) =
            test_engine(Mission::gather(MissionId(1), "Fishing"), state.clone());
        engine.state.mission.permissions = PermissionSet::MOVE;
        assert!(engine.tick_mission().await);
        assert!(rx.try_recv().is_err());
        engine.state.mission.permissions = PermissionSet::GATHER;
        engine.state.authoritative.capabilities.can_fish = true;
        engine.state.authoritative.session.character_guid = Some(1);
        engine.state.authoritative.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = rx.recv().await.unwrap() else {
            panic!("expected fish cast")
        };
        assert_eq!(action.command(), &GameplayCommand::Fish);
        engine.state.authoritative.entities.0.insert(
            EntityId(31),
            wow_state::entities::EntityState {
                id: EntityId(31),
                kind: wow_state::entities::EntityKind::GameObject,
                name: Some("Fishing Bobber".into()),
                target: Some(EntityId(1)),
                ..Default::default()
            },
        );
        // The owned bobber is used after the cast attempt enters its wait phase.
        engine.fishing_cast_at = Some(Instant::now() - Duration::from_secs(1));
        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = rx.recv().await.unwrap() else {
            panic!("expected owned bobber use")
        };
        assert_eq!(action.command(), &GameplayCommand::Gather(EntityId(31)));
        assert!(engine.tick_mission().await);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn named_battleground_waits_when_only_random_queueing_is_supported() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        let mission = Mission {
            id: MissionId(4),
            intent: MissionIntent::Battleground {
                battleground: Some("Warsong Gulch".into()),
            },
            permissions: PermissionSet::MOVE | PermissionSet::COMBAT,
        };
        let (mut engine, mut proxy) = test_engine(mission, authoritative);

        assert!(engine.tick_mission().await);
        assert!(
            engine
                .last_wait_reason
                .as_deref()
                .is_some_and(|reason| { reason.contains("is not supported by the queue command") })
        );
        assert!(proxy.try_recv().is_err());
    }

    #[tokio::test]
    async fn random_battleground_checks_queue_status_before_joining() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        let mission = Mission {
            id: MissionId(4),
            intent: MissionIntent::Battleground { battleground: None },
            permissions: PermissionSet::GROUP,
        };
        let (mut engine, mut proxy) = test_engine(mission, authoritative);

        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = proxy.recv().await.unwrap() else {
            panic!("expected battleground status query")
        };
        assert_eq!(action.command(), &GameplayCommand::BattlegroundStatus);

        engine.battleground_status_requested_at =
            Some(Instant::now() - BATTLEGROUND_QUEUE_START_DELAY);
        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = proxy.recv().await.unwrap() else {
            panic!("expected random queue request")
        };
        assert_eq!(action.command(), &GameplayCommand::BattlegroundJoinRandom);
    }

    #[tokio::test]
    async fn active_battleground_mission_dispatches_only_to_observed_hostile_players() {
        use wow_state::{battleground::BattlegroundQueueStatus, entities::EntityKind};

        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.capabilities.class_id = Some(1);
        authoritative.capabilities.specialization_tree = Some(0);
        authoritative.position.player = Some(WorldPosition {
            map: 489,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        authoritative.entities.0.insert(
            EntityId(1),
            wow_state::entities::EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                health: Some((100, 100)),
                power_type: Some(1),
                power: Some((20, 100)),
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            EntityId(2),
            wow_state::entities::EntityState {
                id: EntityId(2),
                kind: EntityKind::Player,
                hostile: true,
                health: Some((100, 100)),
                position: Some(WorldPosition {
                    map: 489,
                    point: Vec3::new(4.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        authoritative.battleground.queues.insert(
            0,
            wow_state::battleground::BattlegroundQueueState {
                queue_slot: 0,
                battleground_type_id: Some(2),
                status: BattlegroundQueueStatus::InProgress,
                map_id: Some(489),
                invite_timeout_ms: None,
                elapsed_time_ms: Some(10_000),
                auto_leave_time_ms: None,
                team_alliance: Some(true),
            },
        );
        let mission = Mission {
            id: MissionId(4),
            intent: MissionIntent::Battleground { battleground: None },
            permissions: PermissionSet::GROUP | PermissionSet::COMBAT,
        };
        let (mut engine, mut proxy) = test_engine(mission, authoritative);
        engine.battleground_status_retry_after = Some(Instant::now() + Duration::from_secs(20));

        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = proxy.try_recv().unwrap_or_else(|error| {
            panic!(
                "expected a combat action; reason={:?}, dispatch={:?}, error={error:?}",
                engine.last_wait_reason, engine.last_dispatch
            )
        }) else {
            panic!("expected battleground combat action")
        };
        assert!(matches!(
            action.command(),
            GameplayCommand::Attack(EntityId(2))
                | GameplayCommand::Cast {
                    target: Some(EntityId(2)),
                    ..
                }
        ));
    }

    #[tokio::test]
    async fn group_mission_passes_a_fresh_allowed_loot_roll_once() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.group.loot_method = Some(wow_state::group::GroupLootMethod::NeedBeforeGreed);
        let mission = Mission {
            id: MissionId(5),
            intent: MissionIntent::Party {
                role: wow_domain::GroupRole::Auto,
            },
            permissions: PermissionSet::GROUP | PermissionSet::COMBAT | PermissionSet::LOOT,
        };
        let (mut engine, mut proxy) = test_engine(mission, authoritative);
        let request = wow_state::group::GroupLootRollRequest {
            item: EntityId(700),
            map_id: 571,
            item_slot: 2,
            item_id: 1234,
            item_count: 1,
            countdown_ms: 30_000,
            vote_mask: 0b1001,
        };
        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::GroupLootRollStarted(request),
            ))
            .await;

        let WorkerToProxy::Action(query) = proxy.try_recv().expect("expected item metadata query")
        else {
            panic!("expected a metadata query")
        };
        assert_eq!(query.command(), &GameplayCommand::QueryItem { item: 1234 });
        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::ItemTemplate {
                    item: 1234,
                    metadata: wow_state::inventory::ItemTemplateMetadata {
                        quality: 2,
                        ..Default::default()
                    },
                },
            ))
            .await;

        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = proxy.try_recv().expect("expected pass vote") else {
            panic!("expected a loot roll vote")
        };
        assert_eq!(
            action.command(),
            &GameplayCommand::LootRollVote {
                item: EntityId(700),
                item_slot: 2,
                choice: LootRollChoice::Pass,
            }
        );
        assert!(engine.tick_mission().await);
        assert!(
            proxy.try_recv().is_err(),
            "the same live roll is voted once"
        );
    }

    #[tokio::test]
    async fn group_mission_does_not_vote_after_the_observed_roll_expires() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.group.loot_method = Some(wow_state::group::GroupLootMethod::NeedBeforeGreed);
        authoritative
            .group
            .loot_rolls
            .push(wow_state::group::GroupLootRollRequest {
                item: EntityId(701),
                map_id: 571,
                item_slot: 2,
                item_id: 1235,
                item_count: 1,
                countdown_ms: 100,
                vote_mask: 0b1001,
            });
        let mission = Mission {
            id: MissionId(5),
            intent: MissionIntent::Party {
                role: wow_domain::GroupRole::Auto,
            },
            permissions: PermissionSet::GROUP | PermissionSet::COMBAT | PermissionSet::LOOT,
        };
        let (mut engine, mut proxy) = test_engine(mission, authoritative);
        engine
            .group_loot_roll_observed_at
            .insert(EntityId(701), Instant::now() - Duration::from_millis(101));

        assert!(engine.tick_mission().await);
        assert!(proxy.try_recv().is_err());
    }

    #[tokio::test]
    async fn configured_group_loot_needs_only_a_proven_usable_upgrade() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.capabilities.class_id = Some(1);
        authoritative.capabilities.specialization_tree = Some(0);
        authoritative.group.loot_method = Some(wow_state::group::GroupLootMethod::NeedBeforeGreed);
        authoritative.inventory.equipment_authoritative = true;
        authoritative.inventory.equipment_slots_authoritative = true;
        authoritative.inventory.equipped_items.insert(0, 100);
        authoritative.inventory.item_metadata.insert(
            100,
            wow_state::inventory::ItemTemplateMetadata {
                item_class: 4,
                subclass: 2,
                inventory_type: 1,
                allowable_class: u32::MAX,
                item_level: 1,
                required_level: 1,
                ..Default::default()
            },
        );
        authoritative.inventory.item_metadata.insert(
            1234,
            wow_state::inventory::ItemTemplateMetadata {
                item_class: 4,
                subclass: 2,
                quality: 2,
                inventory_type: 1,
                allowable_class: u32::MAX,
                item_level: 80,
                required_level: 1,
                ..Default::default()
            },
        );
        authoritative.entities.0.insert(
            EntityId(1),
            wow_state::entities::EntityState {
                id: EntityId(1),
                kind: wow_state::entities::EntityKind::Player,
                level: Some(60),
                ..Default::default()
            },
        );
        let mission = Mission {
            id: MissionId(5),
            intent: MissionIntent::Party {
                role: wow_domain::GroupRole::Auto,
            },
            permissions: PermissionSet::GROUP | PermissionSet::COMBAT | PermissionSet::LOOT,
        };
        let (mut engine, mut proxy) = test_engine(mission, authoritative);
        engine.runtime_tuning.group.loot.need_usable_upgrades = true;
        engine
            .handle(LaneMessage::Observation(
                ProtocolObservation::GroupLootRollStarted(wow_state::group::GroupLootRollRequest {
                    item: EntityId(702),
                    map_id: 571,
                    item_slot: 2,
                    item_id: 1234,
                    item_count: 1,
                    countdown_ms: 30_000,
                    vote_mask: 0b0011,
                }),
            ))
            .await;

        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = proxy.try_recv().expect("expected need vote") else {
            panic!("expected a loot roll vote")
        };
        assert_eq!(
            action.command(),
            &GameplayCommand::LootRollVote {
                item: EntityId(702),
                item_slot: 2,
                choice: LootRollChoice::Need,
            }
        );
    }

    #[tokio::test]
    async fn goal_mission_dispatches_grounded_active_quest_work_through_shared_validator() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.quests.active.insert(
            42,
            wow_state::quests::QuestProgress {
                complete: false,
                objectives: vec![0],
            },
        );
        let mission = Mission::goal(MissionId(5), "complete my active objective");
        let (mut engine, mut proxy) = test_engine(mission, authoritative);

        assert!(engine.tick_mission().await);
        let WorkerToProxy::Action(action) = proxy.recv().await.unwrap() else {
            panic!("expected shared quest action")
        };
        assert_eq!(action.command(), &GameplayCommand::QueryQuest { quest: 42 });
        assert_eq!(action.origin(), PlanOrigin::SystemPolicy);
    }

    #[tokio::test]
    async fn inoculation_waits_for_item_spell_metadata_before_shared_targeted_use() {
        use wow_state::{
            inventory::{InventoryItemInstance, ItemTemplateMetadata},
            quests::{
                QuestDefinition, QuestItemObjective, QuestProgress, QuestTargetKind,
                QuestTargetObjective,
            },
        };

        let position = WorldPosition {
            map: 530,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(1);
        authoritative.position.player = Some(position);
        authoritative.quests.active.insert(
            9303,
            QuestProgress {
                complete: false,
                objectives: vec![0],
            },
        );
        authoritative.quests.definitions.insert(
            9303,
            QuestDefinition {
                quest: 9303,
                title: "Inoculation".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: vec![QuestTargetObjective {
                    slot: 0,
                    kind: QuestTargetKind::Creature,
                    entry: 16534,
                    required: 6,
                    item_drop: 0,
                    text: "Nestlewood Owlkin inoculated".into(),
                }],
                items: vec![QuestItemObjective {
                    item: 22962,
                    required: 1,
                }],
            },
        );
        authoritative.inventory.instances_authoritative = true;
        authoritative.inventory.instances.insert(
            EntityId(92),
            InventoryItemInstance {
                item: 22962,
                guid: EntityId(92),
                backpack_slot: 23,
                count: 1,
            },
        );
        authoritative.entities.0.insert(
            EntityId(93),
            wow_state::entities::EntityState {
                id: EntityId(93),
                entry: 16518,
                kind: wow_state::entities::EntityKind::Unit,
                position: Some(WorldPosition {
                    point: Vec3::new(4.0, 0.0, 0.0),
                    ..position
                }),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(93)), authoritative);

        assert!(engine.tick_incomplete_quest(9303).await);
        let WorkerToProxy::Action(query) = proxy.recv().await.unwrap() else {
            panic!("missing item metadata must request the item template")
        };
        assert_eq!(query.command(), &GameplayCommand::QueryItem { item: 22962 });

        engine.state.authoritative.inventory.item_metadata.insert(
            22962,
            ItemTemplateMetadata {
                use_spell_id: 29529,
                ..Default::default()
            },
        );
        assert!(engine.tick_incomplete_quest(9303).await);
        assert!(proxy.try_recv().is_err());
        assert!(
            engine
                .last_wait_reason
                .as_deref()
                .is_some_and(|reason| { reason.contains("does not match grounded quest spell") })
        );

        engine
            .state
            .authoritative
            .inventory
            .item_metadata
            .get_mut(&22962)
            .unwrap()
            .use_spell_id = 29528;
        assert!(engine.tick_incomplete_quest(9303).await);
        let WorkerToProxy::Action(use_item) = proxy.recv().await.unwrap() else {
            panic!("matching authoritative template should allow shared item use")
        };
        assert_eq!(
            use_item.command(),
            &GameplayCommand::UseItemInstance {
                item: 22962,
                item_guid: EntityId(92),
                backpack_slot: 23,
                spell: 29528,
                target: Some(EntityId(93)),
                cast_count: 1,
            }
        );
        assert!(matches!(
            engine.pending_quest_action,
            Some(PendingQuestAction::QuestCredit {
                quest: 9303,
                objective: 0,
                target: EntityId(93),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn goal_mission_does_not_treat_free_form_text_as_action_authority() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        let (mut engine, mut proxy) = test_engine(
            Mission::goal(MissionId(6), "attack a nearby stranger"),
            authoritative,
        );

        assert!(engine.tick_mission().await);
        assert!(
            engine
                .last_wait_reason
                .as_deref()
                .is_some_and(|reason| { reason.contains("waiting for grounded supported work") })
        );
        assert!(proxy.try_recv().is_err());
    }

    #[tokio::test]
    async fn goal_quest_work_remains_blocked_when_mission_lacks_quest_permission() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.quests.active.insert(
            42,
            wow_state::quests::QuestProgress {
                complete: false,
                objectives: vec![0],
            },
        );
        let mission = Mission {
            id: MissionId(7),
            intent: MissionIntent::Goal {
                text: "complete my active objective".into(),
            },
            permissions: PermissionSet::MOVE,
        };
        let (mut engine, mut proxy) = test_engine(mission, authoritative);

        assert!(engine.tick_mission().await);
        assert_eq!(engine.last_dispatch, DispatchOutcome::Rejected);
        assert!(proxy.try_recv().is_err());
    }

    #[test]
    fn corpse_route_defers_when_the_corpse_map_does_not_match() {
        let corpse = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut entities = BTreeMap::new();
        entities.insert(
            EntityId(2),
            wow_state::entities::EntityState {
                id: EntityId(2),
                hostile: true,
                position: Some(WorldPosition { map: 2, ..corpse }),
                ..Default::default()
            },
        );
        let ghost = WorldPosition { map: 2, ..corpse };
        assert!(!corpse_route_map_matches(ghost, corpse));
        assert!(corpse_reclaim_is_safe(&entities, EntityId(1), corpse));
    }

    #[test]
    fn corpse_reclaim_waits_for_nearby_living_hostile_risk_to_clear() {
        let corpse = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut entities = BTreeMap::new();
        entities.insert(
            EntityId(2),
            wow_state::entities::EntityState {
                id: EntityId(2),
                hostile: true,
                position: Some(WorldPosition {
                    point: Vec3::new(10.0, 0.0, 0.0),
                    ..corpse
                }),
                ..Default::default()
            },
        );
        assert!(!corpse_reclaim_is_safe(&entities, EntityId(1), corpse));
        entities.get_mut(&EntityId(2)).unwrap().mark_dead();
        assert!(corpse_reclaim_is_safe(&entities, EntityId(1), corpse));
    }

    #[test]
    fn corpse_reclaim_attempts_are_bounded_and_reset_on_resurrection() {
        let (mut engine, _) = test_engine(
            Mission::quest(MissionId(1)),
            wow_state::AuthoritativeState::default(),
        );
        engine.corpse_reclaim_attempts = MAX_CORPSE_RECLAIM_ATTEMPTS;
        assert!(!corpse_reclaim_attempt_allowed(
            engine.corpse_reclaim_attempts
        ));
        engine.corpse_reclaim_attempts -= 1;
        assert!(corpse_reclaim_attempt_allowed(
            engine.corpse_reclaim_attempts
        ));
        engine.corpse_reclaim_attempts = 0; // The live-player tick clears this counter.
        assert_eq!(engine.corpse_reclaim_attempts, 0);
    }

    #[test]
    fn observed_resurrection_is_not_treated_as_dead() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.character_guid = Some(1);
        authoritative.entities.0.insert(
            EntityId(1),
            wow_state::entities::EntityState {
                id: EntityId(1),
                health: Some((1, 100)),
                ..Default::default()
            },
        );
        let (engine, _) = test_engine(Mission::quest(MissionId(1)), authoritative);
        assert!(!engine.player_is_dead());
    }

    #[tokio::test]
    async fn death_recovery_stops_and_clears_active_quest_movement() {
        let player = EntityId(1);
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.in_world = true;
        authoritative.session.character_guid = Some(player.0);
        authoritative.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        authoritative.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                health: Some((0, 100)),
                ..Default::default()
            },
        );
        let (mut engine, mut proxy) = test_engine(Mission::quest(MissionId(1)), authoritative);
        engine.queue_movement(
            Vec3::new(20.0, 0.0, 0.0),
            1.0,
            None,
            None,
            PlanOrigin::SystemPolicy,
            QuestWorkRuntime {
                id: QuestWorkId(7),
                key: QuestWorkKey::AcquireQuest { quest: None },
            },
            MovementPurpose::SearchArea,
        );

        assert!(engine.tick_mission().await);

        assert!(engine.pending_movement.is_none());
        assert!(engine.current_work.is_none());
        let WorkerToProxy::Action(action) = proxy.try_recv().expect("stop movement action") else {
            panic!("expected a stop movement action")
        };
        assert_eq!(action.command(), &GameplayCommand::StopMovement);
    }

    #[test]
    fn ghost_aura_triggers_death_recovery_without_zero_health() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative.session.character_guid = Some(1);
        authoritative.entities.0.insert(
            EntityId(1),
            wow_state::entities::EntityState {
                id: EntityId(1),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        authoritative
            .auras
            .by_entity
            .entry(EntityId(1))
            .or_default()
            .insert(
                0,
                wow_state::auras::AuraInstance {
                    slot: 0,
                    spell: wow_state::life::GHOST_AURA_SPELL_ID,
                    positive: Some(true),
                    caster: None,
                    max_duration_ms: None,
                    remaining_ms: None,
                    observed_at_ms: None,
                },
            );

        let (engine, _) = test_engine(Mission::quest(MissionId(1)), authoritative);

        assert_eq!(engine.player_life_status(), PlayerLifeStatus::Ghost);
        assert!(engine.player_is_dead());
    }

    #[test]
    fn quest_tool_activation_waits_for_temporary_item_target() {
        let (mut engine, _proxy) = combat_engine();
        engine.state.authoritative.quests.active.insert(
            10584,
            wow_state::quests::QuestProgress {
                complete: false,
                objectives: vec![0, 0, 0, 0],
            },
        );
        engine.state.authoritative.quests.definitions.insert(
            10584,
            wow_state::quests::QuestDefinition {
                quest: 10584,
                title: "Picking Up Some Power Converters".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: vec![wow_state::quests::QuestTargetObjective {
                    slot: 0,
                    kind: wow_state::quests::QuestTargetKind::Creature,
                    entry: 21731,
                    required: 5,
                    item_drop: 0,
                    text: "Electromentals collected".into(),
                }],
                items: vec![],
            },
        );
        engine.current_work = Some(QuestWorkRuntime {
            id: QuestWorkId(1),
            key: QuestWorkKey::InteractObjective {
                quest: 10584,
                objective: usize::MAX,
                target: EntityId(80),
            },
        });
        engine.pending_quest_action = Some(PendingQuestAction::ControlActivation {
            target: EntityId(80),
            started: Instant::now(),
        });

        assert!(engine.pending_quest_action_blocks());
        assert!(engine.pending_quest_action.is_some());

        engine.state.authoritative.entities.0.insert(
            EntityId(81),
            wow_state::entities::EntityState {
                id: EntityId(81),
                entry: 21729,
                kind: wow_state::entities::EntityKind::Unit,
                position: Some(wow_domain::WorldPosition {
                    map: 0,
                    point: Vec3::new(3.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                health: Some((100, 100)),
                ..Default::default()
            },
        );

        assert!(!engine.pending_quest_action_blocks());
        assert!(engine.pending_quest_action.is_none());
    }

    #[tokio::test]
    async fn llm_proposal_uses_the_shared_action_validator() {
        let (mut engine, mut proxy) = combat_engine();
        let action = ProposedAction {
            id: ActionId(41),
            task: TaskId(42),
            origin: PlanOrigin::Llm,
            stamp: engine.state.stamp(),
            command: GameplayCommand::Attack(EntityId(9)),
        };

        assert!(engine.handle(LaneMessage::Propose(action)).await);
        let WorkerToProxy::Action(validated) = proxy.recv().await.unwrap() else {
            panic!("expected validated LLM proposal")
        };
        assert_eq!(validated.command(), &GameplayCommand::Attack(EntityId(9)));
        assert_eq!(validated.origin(), PlanOrigin::Llm);
    }

    #[tokio::test]
    async fn world_exit_discards_pending_work_and_keeps_mission() {
        let mission = Mission::quest(MissionId(9));
        let (mut engine, _proxy_rx) = test_engine(mission, Default::default());
        let destination = Vec3::new(10.0, 0.0, 0.0);
        let work = QuestWorkRuntime {
            id: QuestWorkId(1),
            key: QuestWorkKey::TravelToObjective {
                quest: 42,
                objective: 0,
                destination: WorldPosition {
                    map: 0,
                    point: destination,
                    orientation: 0.0,
                },
            },
        };
        engine.current_work = Some(work.clone());
        engine.queue_movement(
            destination,
            4.0,
            Some(GameplayCommand::Interact(EntityId(8))),
            None,
            PlanOrigin::SystemPolicy,
            work,
            MovementPurpose::ApproachGroundedTarget,
        );
        engine.pending_quest_action = Some(PendingQuestAction::Interact {
            target: EntityId(8),
            started: Instant::now(),
        });
        engine.pending_accept = Some((42, EntityId(8), Instant::now()));
        engine.credited_quest_targets.insert((42, 0, EntityId(8)));
        engine.los_blocked.insert(EntityId(8));
        assert_eq!(
            engine.defer_quest_interaction(42, EntityId(8)),
            (1, Duration::from_secs(5))
        );

        assert!(
            engine
                .handle(LaneMessage::Observation(ProtocolObservation::LeftWorld))
                .await
        );
        assert!(engine.pending_movement.is_none());
        assert!(engine.pending_quest_action.is_none());
        assert!(engine.pending_accept.is_none());
        assert_eq!(
            engine.defer_quest_interaction(42, EntityId(8)),
            (2, Duration::from_secs(10))
        );
        assert!(engine.current_work.is_none());
        assert!(engine.credited_quest_targets.is_empty());
        assert!(engine.los_blocked.is_empty());
        assert!(matches!(engine.state.mission.intent, MissionIntent::Quest));

        assert!(
            engine
                .handle(LaneMessage::Observation(
                    ProtocolObservation::EnteredWorld {
                        character_guid: 7,
                        position: None
                    }
                ))
                .await
        );
        assert!(engine.pending_movement.is_none());
        assert!(engine.current_work.is_none());
    }

    #[test]
    fn dialog_states_match_azerothcore_335a() {
        for status in [2, 4, 7, 8] {
            assert!(quest_status_available(status));
        }
        for status in [3, 6, 9, 10] {
            assert!(quest_status_reward(status));
        }
    }

    #[test]
    fn turn_in_search_continues_past_the_old_eighteen_yard_envelope() {
        let destination = Vec3::new(0.0, 0.0, 20.0);
        assert!(!turn_in_search_arrived(
            Vec3::new(17.8, 0.0, 0.0),
            destination
        ));
        assert!(!turn_in_search_arrived(
            Vec3::new(4.9, 0.0, 0.0),
            destination
        ));
        assert!(turn_in_search_arrived(
            Vec3::new(2.0, 0.0, 17.0),
            destination
        ));
    }

    #[test]
    fn unreachable_turn_in_hint_stays_blocked_until_player_moves_or_changes_map() {
        let (mut engine, _proxy_rx) = test_engine(Mission::quest(MissionId(9)), Default::default());
        let failed_area = WorldPosition {
            map: 0,
            point: Vec3::new(-5732.0, -367.0, 366.0),
            orientation: 0.0,
        };
        engine.record_turn_in_search_failure(5541, failed_area);

        assert!(engine.turn_in_search_hint_blocked(5541, failed_area));
        assert!(engine.turn_in_search_failures.contains_key(&5541));

        let moved = WorldPosition {
            point: Vec3::new(-5670.0, -367.0, 366.0),
            ..failed_area
        };
        assert!(!engine.turn_in_search_hint_blocked(5541, moved));
        assert!(!engine.turn_in_search_failures.contains_key(&5541));

        engine.record_turn_in_search_failure(5541, failed_area);
        let other_map = WorldPosition {
            map: 1,
            ..failed_area
        };
        assert!(!engine.turn_in_search_hint_blocked(5541, other_map));
        assert!(!engine.turn_in_search_failures.contains_key(&5541));
    }

    #[test]
    fn static_search_advances_after_arrival_and_sets_a_short_retry() {
        let (mut engine, _proxy_rx) = test_engine(Mission::quest(MissionId(9)), Default::default());
        let key = (456, 0, 0);
        let first = Vec3::new(1.0, 0.0, 0.0);
        let second = Vec3::new(2.0, 0.0, 0.0);
        let candidates = vec![first, second];

        assert_eq!(
            engine.next_search_destination(key, &candidates, None),
            Some(first)
        );
        assert_eq!(
            engine.next_search_destination(key, &candidates, Some(first)),
            Some(second)
        );
        assert_eq!(
            engine.next_search_destination(key, &candidates, Some(second)),
            None
        );
        let retry_after = engine.search_retry_after[&key];
        assert!(retry_after > Instant::now());
        assert!(retry_after <= Instant::now() + Duration::from_secs(5));
    }

    #[test]
    fn incomplete_quest_selection_skips_quests_during_search_retry() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative
            .quests
            .active
            .insert(182, wow_state::quests::QuestProgress::default());
        authoritative
            .quests
            .active
            .insert(3361, wow_state::quests::QuestProgress::default());
        let (mut engine, _proxy_rx) = test_engine(Mission::quest(MissionId(10)), authoritative);
        engine
            .search_retry_after
            .insert((182, 0, 0), Instant::now() + Duration::from_secs(30));

        assert_eq!(engine.next_incomplete_quest(), Some(3361));

        engine
            .search_retry_after
            .insert((3361, 0, 0), Instant::now() + Duration::from_secs(30));
        assert_eq!(engine.next_incomplete_quest(), Some(182));
    }

    #[test]
    fn far_apart_collection_quest_keeps_focus_while_search_is_retrying() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative
            .quests
            .active
            .insert(182, wow_state::quests::QuestProgress::default());
        authoritative
            .quests
            .active
            .insert(3361, wow_state::quests::QuestProgress::default());
        let (mut engine, _proxy_rx) = test_engine(Mission::quest(MissionId(10)), authoritative);
        let nearby = WorldPosition {
            map: 0,
            point: Vec3::new(-6509.0, 301.0, 0.0),
            orientation: 0.0,
        };
        let far = WorldPosition {
            map: 0,
            point: Vec3::new(-6375.0, 774.0, 0.0),
            orientation: 0.0,
        };
        assert!(nearby.point.distance(far.point) > 400.0);

        engine.set_work(QuestWorkKey::CollectItem {
            quest: 3361,
            item: 10438,
        });
        engine.search_retry_after.insert(
            (3361, usize::MAX, 10438),
            Instant::now() + Duration::from_secs(5),
        );

        assert_eq!(engine.next_incomplete_quest(), Some(3361));
    }

    #[test]
    fn quest_item_gameobject_uses_the_captured_open_command() {
        let target = EntityId(0xF110_0244_1300_0C92);
        assert_eq!(
            quest_item_gameobject_open_command(target),
            GameplayCommand::CastGameObject {
                spell: QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID,
                target,
                report_use: true,
            }
        );
    }

    #[test]
    fn quest_navigation_position_prefers_controlled_mover_over_player() {
        let (mut engine, _proxy_rx) =
            test_engine(Mission::quest(MissionId(11)), Default::default());
        let player = WorldPosition {
            map: 0,
            point: Vec3::new(10.0, 20.0, 0.0),
            orientation: 0.0,
        };
        let mover = WorldPosition {
            map: 1,
            point: Vec3::new(100.0, 200.0, 5.0),
            orientation: 1.0,
        };
        engine.state.authoritative.position.player = Some(player);
        engine.state.authoritative.control.mover = Some(EntityId(99));
        engine.state.authoritative.control.mover_position = Some(mover);

        assert_eq!(engine.active_mover_position(), Some(mover));
    }

    #[test]
    fn genuine_search_path_failure_clears_focus_and_allows_quest_selection_change() {
        let mut authoritative = wow_state::AuthoritativeState::default();
        authoritative
            .quests
            .active
            .insert(182, wow_state::quests::QuestProgress::default());
        authoritative
            .quests
            .active
            .insert(3361, wow_state::quests::QuestProgress::default());
        let (mut engine, _proxy_rx) = test_engine(Mission::quest(MissionId(10)), authoritative);

        let failed_search_work = engine.set_work(QuestWorkKey::CollectItem {
            quest: 3361,
            item: 10438,
        });
        assert_eq!(engine.next_incomplete_quest(), Some(3361));

        // The production no-route branch clears this work after repeated
        // failures. Without that focus, normal quest ordering can choose 182.
        engine.clear_quest_search_focus(failed_search_work.id);
        assert_eq!(engine.next_incomplete_quest(), Some(182));
    }

    #[test]
    fn static_search_candidates_are_distinct_local_and_bounded() {
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let candidates = bounded_candidates(
            vec![
                Vec3::new(9.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, 0.0),
                Vec3::new(5.0, 0.0, 0.0),
                Vec3::new(5.0, 0.0, 0.0),
                Vec3::new(300.0, 0.0, 0.0),
            ],
            origin,
        );
        assert_eq!(candidates.len(), 5);
        assert_eq!(candidates[0], Vec3::new(1.0, 0.0, 0.0));
        assert!(!candidates.contains(&Vec3::new(300.0, 0.0, 0.0)));
    }

    #[test]
    fn navigation_failures_separate_path_errors_from_temporary_planner_errors() {
        for error in [
            wow_navigation::NavigationError::InvalidCoordinate,
            wow_navigation::NavigationError::MissingNavigationData,
            wow_navigation::NavigationError::NoRoute,
            wow_navigation::NavigationError::FloorDiscontinuity,
            wow_navigation::NavigationError::RetryExhausted,
        ] {
            assert!(is_terminal_navigation_failure(&error), "{error:?}");
        }
        for error in [
            wow_navigation::NavigationError::RoutePlannerBusy,
            wow_navigation::NavigationError::RoutePlanningCancelled,
            wow_navigation::NavigationError::RoutePlanningDeadlineExceeded,
        ] {
            assert!(!is_terminal_navigation_failure(&error), "{error:?}");
        }
    }

    #[test]
    fn movement_failure_uses_bounded_stall_and_operation_limits() {
        assert_eq!(
            movement_failure(Duration::from_secs(4), Duration::from_secs(4)),
            None
        );
        assert_eq!(
            movement_failure(Duration::from_secs(5), MOVEMENT_STALL_TIMEOUT),
            Some(MovementFailure::Stalled)
        );
        assert_eq!(
            movement_failure(MOVEMENT_OPERATION_TIMEOUT, Duration::from_secs(4)),
            Some(MovementFailure::TimedOut)
        );
        assert_eq!(
            movement_failure(MOVEMENT_OPERATION_TIMEOUT, MOVEMENT_STALL_TIMEOUT),
            Some(MovementFailure::TimedOut)
        );
    }

    #[test]
    fn item_source_search_keeps_same_map_sources_beyond_local_radius() {
        let origin = Vec3::default();
        let distant_source = Vec3::new(280.0, 380.0, 0.0);

        assert!(bounded_candidates(vec![distant_source], origin).is_empty());
        assert_eq!(
            bounded_candidates_within(
                vec![distant_source],
                origin,
                QUEST_ITEM_SOURCE_SEARCH_RADIUS,
            ),
            vec![distant_source]
        );
    }

    #[test]
    fn exhausted_search_selects_a_nearby_roam_waypoint() {
        let (mut engine, _proxy_rx) =
            test_engine(Mission::quest(MissionId(11)), Default::default());
        let origin = Vec3::new(1.0, 2.0, 3.0);
        let destination = engine.next_search_roam_destination((182, 0, 0), origin);

        assert!((destination.distance(origin) - QUEST_SEARCH_ROAM_RADIUS).abs() < 0.001);
        assert_eq!(destination.z, origin.z);
    }

    #[test]
    fn quest_search_arrival_uses_the_movement_range() {
        let origin = Vec3::new(0.0, 0.0, 0.0);
        assert!(quest_search_arrived(
            origin,
            Vec3::new(QUEST_SEARCH_ARRIVAL_RANGE, 0.0, 0.0)
        ));
        assert!(!quest_search_arrived(
            origin,
            Vec3::new(QUEST_SEARCH_ARRIVAL_RANGE + 0.1, 0.0, 0.0)
        ));
    }

    #[test]
    fn quest_tool_search_arrival_uses_its_movement_range() {
        let origin = Vec3::new(0.0, 0.0, 0.0);
        assert!(quest_tool_search_arrived(
            origin,
            Vec3::new(QUEST_TOOL_SEARCH_RANGE, 0.0, 0.0)
        ));
        assert!(!quest_tool_search_arrived(
            origin,
            Vec3::new(QUEST_TOOL_SEARCH_RANGE + 0.1, 0.0, 0.0)
        ));
    }
}
