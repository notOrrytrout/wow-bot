use super::{LaneMessage, LaneState};
use crate::action::finalize;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use wow_control_proto::WorkerToProxy;
use wow_domain::*;
use wow_infra::logging::structured::{DiagnosticLogger, DiagnosticStream};
use wow_policy::questing::{
    objectives::{ObjectiveResolution, resolve as resolve_objective, resolve_with_exclusions},
    tasks::{QuestWorkId, QuestWorkKey, QuestWorkRuntime},
};
use wow_state::{ProtocolObservation, Snapshot, reduce};

#[derive(Clone, Debug)]
struct PendingMovement {
    destination: Vec3,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MovementPurpose {
    SearchArea,
    ApproachGroundedTarget,
    SurvivalApproach,
}

const TURN_IN_SEARCH_RANGE: f32 = 5.0;
const QUEST_SEARCH_ARRIVAL_RANGE: f32 = 18.0;
const QUEST_TOOL_SEARCH_RANGE: f32 = 12.0;

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
    Rejected,
    TransportClosed,
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
    last_turn_in_search_query: Option<(u32, Instant)>,
    pending_movement: Option<PendingMovement>,
    pending_quest_action: Option<PendingQuestAction>,
    interaction_retry_after: BTreeMap<(u32, EntityId), (u8, Instant)>,
    search_attempts: BTreeMap<(u32, usize, u32), BTreeSet<(u32, u32, u32)>>,
    search_retry_after: BTreeMap<(u32, usize, u32), Instant>,
    current_work: Option<QuestWorkRuntime>,
    last_wait_reason: Option<String>,
    last_dispatch: DispatchOutcome,
    movement_controller: Option<wow_navigation::MovementController>,
    diagnostics: Option<DiagnosticLogger>,
    credited_quest_targets: BTreeSet<(u32, usize, EntityId)>,
    los_blocked: BTreeSet<EntityId>,
    los_attempts: BTreeMap<EntityId, u8>,
    server_range_recovery: BTreeMap<EntityId, (crate::action::spatial::ServerRangeCorrection, u8)>,
    behind_reposition_pending: BTreeSet<(u32, EntityId)>,
    behind_retry_after: BTreeMap<(u32, EntityId), Instant>,
    behind_retry_cast_allowed: BTreeSet<(u32, EntityId)>,
    maintenance_retry_after: BTreeMap<(u32, EntityId), Instant>,
    last_maintenance_tick: Option<Instant>,
    last_maintenance_status: Option<String>,
    post_combat_loot: Option<(EntityId, Instant, Option<wow_state::entities::EntityState>)>,
    last_recovery_action: Option<Instant>,
    last_survival_action: Option<(EntityId, Instant)>,
}

impl LaneEngine {
    pub fn new(
        state: LaneState,
        rx: mpsc::Receiver<LaneMessage>,
        proxy: mpsc::Sender<WorkerToProxy>,
    ) -> Self {
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
            last_turn_in_search_query: None,
            pending_movement: None,
            pending_quest_action: None,
            interaction_retry_after: BTreeMap::new(),
            search_attempts: BTreeMap::new(),
            search_retry_after: BTreeMap::new(),
            current_work: None,
            last_wait_reason: None,
            last_dispatch: DispatchOutcome::Rejected,
            movement_controller: None,
            diagnostics: None,
            credited_quest_targets: BTreeSet::new(),
            los_blocked: BTreeSet::new(),
            los_attempts: BTreeMap::new(),
            server_range_recovery: BTreeMap::new(),
            behind_reposition_pending: BTreeSet::new(),
            behind_retry_after: BTreeMap::new(),
            behind_retry_cast_allowed: BTreeSet::new(),
            maintenance_retry_after: BTreeMap::new(),
            last_maintenance_tick: None,
            last_maintenance_status: None,
            post_combat_loot: None,
            last_recovery_action: None,
            last_survival_action: None,
        }
    }

    pub fn with_movement_controller(
        mut self,
        controller: wow_navigation::MovementController,
    ) -> Self {
        self.movement_controller = Some(controller);
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

    pub async fn run(mut self) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if !self.tick_mission().await { break; }
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
                if matches!(o, ProtocolObservation::LeftWorld) {
                    self.reset_session_work();
                }
                if let ProtocolObservation::CastFailed {
                    spell,
                    reason,
                    target,
                } = &o
                {
                    if let Some(target) = target {
                        self.maintenance_retry_after.insert(
                            (*spell, *target),
                            wow_policy::maintenance::retry_deadline(Instant::now()),
                        );
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
                | ProtocolObservation::QuestRemoved { quest } = &o
                {
                    if self
                        .pending_turn_in
                        .is_some_and(|(pending_quest, _, _)| pending_quest == *quest)
                    {
                        tracing::info!(lane=?self.state.lane, quest, "quest turn-in step confirmed by authoritative server state");
                        self.pending_turn_in = None;
                    }
                }
                let delta = reduce(&mut self.state.authoritative, o);
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
                self.last_quest_step = None;
                self.pending_accept = None;
                self.pending_turn_in = None;
                self.pending_movement = None;
                self.pending_quest_action = None;
                self.current_work = None;
                self.credited_quest_targets.clear();
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
            self.waiting(format!("lane paused by {:?}", self.state.pause));
            return true;
        }
        if self.state.activation != ActivationStage::Act {
            self.waiting(format!("activation stage is {:?}", self.state.activation));
            return true;
        }
        if !self.state.authoritative.session.in_world {
            self.waiting("configured world session is not yet authoritative".to_owned());
            return true;
        }
        if self.player_is_dead() {
            return self.tick_death_recovery().await;
        }
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        if let Some(attacker) = wow_policy::combat::engagement::survival_attacker(&snapshot) {
            if self
                .pending_movement
                .as_ref()
                .is_some_and(|movement| movement.purpose == MovementPurpose::SurvivalApproach)
            {
                return self.tick_movement().await;
            }
            if self.pending_movement.is_some() {
                tracing::info!(lane=?self.state.lane, ?attacker, "survival attacker preempted voluntary movement");
                let _ = self.propose_recovery(GameplayCommand::StopMovement).await;
                self.pending_movement = None;
                self.current_work = None;
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
            let _ = self.propose_recovery(GameplayCommand::StopMovement).await;
            self.pending_movement = None;
            self.current_work = None;
        }
        if self.pending_movement.is_some() {
            return self.tick_movement().await;
        }
        if self.maintenance_eligible() {
            if let Some(result) = self.tick_maintenance().await {
                return result;
            }
        }
        match self.state.mission.intent.clone() {
            MissionIntent::Idle => true,
            MissionIntent::Quest => self.tick_quest().await,
            MissionIntent::Party { .. } | MissionIntent::Raid { .. } => {
                self.tick_group_encounter().await
            }
            other => {
                self.waiting(format!(
                    "mission scheduler for {other:?} is not implemented yet"
                ));
                true
            }
        }
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
            self.waiting("group mission is waiting for an observed online group member".into());
            return true;
        }

        let Some(target) = wow_policy::group::encounter::from_observed(&snapshot).preferred_target
        else {
            self.waiting("group mission is waiting for an observed encounter target".into());
            return true;
        };
        let Some(target_state) = self.state.authoritative.entities.0.get(&target) else {
            self.waiting(
                "observed group encounter target is not present in current entity state".into(),
            );
            return true;
        };
        if !target_state.hostile || target_state.is_dead() {
            self.waiting("observed group encounter target is not a live hostile".into());
            return true;
        }

        self.dispatch_combat_target(target, false).await
    }

    async fn tick_survival(&mut self, target: EntityId) -> bool {
        if self.last_survival_action.is_some_and(|(previous, at)| {
            previous == target && at.elapsed() < Duration::from_millis(2750)
        }) {
            return true;
        }
        self.dispatch_combat_target(target, true).await
    }

    fn player_is_dead(&self) -> bool {
        let Some(player) = self
            .state
            .authoritative
            .session
            .character_guid
            .map(EntityId)
        else {
            return false;
        };
        self.state
            .authoritative
            .entities
            .0
            .get(&player)
            .is_some_and(wow_state::entities::EntityState::is_dead)
    }

    async fn tick_death_recovery(&mut self) -> bool {
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
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let Some(corpse) = self.state.authoritative.life.corpse else {
            tracing::info!(lane=?self.state.lane,"death recovery releasing spirit and querying corpse");
            let _ = self.propose_recovery(GameplayCommand::ReleaseSpirit).await;
            return self.propose_recovery(GameplayCommand::QueryCorpse).await;
        };
        if corpse.map != player_pos.map {
            self.waiting(format!("death recovery corpse is on map {} while ghost is on map {}; instance/entrance recovery is not yet grounded",corpse.map,player_pos.map));
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
        tracing::info!(lane=?self.state.lane,?player_guid,"death recovery reclaiming corpse");
        self.propose_recovery(GameplayCommand::ReclaimCorpse {
            player: player_guid,
        })
        .await
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

    async fn tick_maintenance(&mut self) -> Option<bool> {
        self.last_maintenance_tick = Some(Instant::now());
        let now = Instant::now();
        self.maintenance_retry_after
            .retain(|_, deadline| *deadline > now);
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        match wow_policy::maintenance::decide_next(
            &snapshot,
            &self.maintenance_retry_after,
            now,
            true,
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
                self.maintenance_retry_after.insert(
                    (spell, target),
                    wow_policy::maintenance::retry_deadline(now),
                );
                Some(
                    self.propose_command(GameplayCommand::MaintainBuff { spell, target }, false)
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
        let now = Instant::now();
        self.pending_movement = Some(PendingMovement {
            destination,
            acceptable_range,
            resume,
            resume_pending,
            resume_origin,
            work,
            last_step: None,
            purpose,
            started_at: now,
            last_progress_at: now,
            last_progress_position: None,
            last_progress_log: None,
        });
    }

    fn queue_search_movement(&mut self, destination: Vec3, work: QuestWorkRuntime) {
        self.queue_movement(
            destination,
            QUEST_SEARCH_ARRIVAL_RANGE,
            None,
            None,
            PlanOrigin::SystemPolicy,
            work,
            MovementPurpose::SearchArea,
        );
    }

    async fn tick_movement(&mut self) -> bool {
        let Some(mut movement) = self.pending_movement.take() else {
            return true;
        };
        if should_supersede_search_movement(
            movement.purpose,
            self.quest_movement_has_live_target(&movement),
        ) {
            tracing::info!(lane=?self.state.lane, work_id=?movement.work.id, key=?movement.work.key, "live authoritative quest target superseded search-area movement");
            let _ = self
                .propose_command(GameplayCommand::StopMovement, false)
                .await;
            self.current_work = None;
            return self.tick_quest().await;
        }
        if movement
            .last_step
            .is_some_and(|at| at.elapsed() < Duration::from_millis(225))
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
        let controlled_mover = self.state.authoritative.control.mover.is_some();
        let movement_flags = if controlled_mover {
            self.state.authoritative.control.movement_flags
        } else {
            self.state.authoritative.position.flags
        };
        let locomotion =
            wow_navigation::LocomotionMode::from_server_flags(controlled_mover, movement_flags);
        let distance = match locomotion {
            wow_navigation::LocomotionMode::Ground => (movement.destination.x - player.point.x)
                .hypot(movement.destination.y - player.point.y),
            wow_navigation::LocomotionMode::Flight => player.point.distance(movement.destination),
        };

        if movement.started_at.elapsed() >= Duration::from_secs(90) {
            tracing::warn!(lane=?self.state.lane, work_id=?movement.work.id, key=?movement.work.key, ?locomotion, remaining=distance, "movement operation timed out");
            self.stop_owned_movement(movement.purpose).await;
            self.current_work = None;
            self.waiting(format!(
                "movement for {:?} timed out; waiting for authoritative state before retry",
                movement.work.key
            ));
            return true;
        }

        match movement.last_progress_position {
            Some(previous) if previous.distance(player.point) >= 0.35 => {
                movement.last_progress_position = Some(player.point);
                movement.last_progress_at = Instant::now();
            }
            None => {
                movement.last_progress_position = Some(player.point);
                movement.last_progress_at = Instant::now();
            }
            _ => {}
        }
        if movement.last_progress_at.elapsed() >= Duration::from_secs(5) {
            tracing::warn!(lane=?self.state.lane, work_id=?movement.work.id, key=?movement.work.key, ?locomotion, remaining=distance, "movement made no authoritative progress; stopping owned movement for deterministic retry");
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
            tracing::info!(lane=?self.state.lane, work_id=?movement.work.id, key=?movement.work.key, ?locomotion, remaining=distance, mover=?self.state.authoritative.control.mover, "movement operation progress");
            self.diagnostic(
                DiagnosticStream::MovementHeartbeat,
                "movement_progress",
                serde_json::json!({
                    "work_id": movement.work.id.0,
                    "locomotion": format!("{locomotion:?}"),
                    "remaining": distance,
                    "mover_controlled": controlled_mover,
                }),
            );
            movement.last_progress_log = Some(Instant::now());
        }
        if distance <= movement.acceptable_range {
            tracing::info!(lane=?self.state.lane, work_id=?movement.work.id, remaining=distance, purpose=?movement.purpose, "owned movement work reached interaction envelope");
            self.record_search_arrival(&movement);
            self.stop_owned_movement(movement.purpose).await;
            self.current_work = None;
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
        let step_result = match &self.movement_controller {
            Some(controller) => controller.next_step(
                player,
                movement.destination,
                movement.acceptable_range,
                locomotion,
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
                        "purpose": format!("{:?}", movement.purpose),
                        "step_remaining": step.remaining,
                    }),
                );
                step.next
            }
            Ok(None) => {
                self.pending_movement = Some(movement);
                return true;
            }
            Err(error) => {
                self.diagnostic(
                    DiagnosticStream::Navigation,
                    "movement_step_rejected",
                    serde_json::json!({
                        "work_id": movement.work.id.0,
                        "locomotion": format!("{locomotion:?}"),
                        "purpose": format!("{:?}", movement.purpose),
                        "reason": format!("{error:?}"),
                    }),
                );
                self.waiting(format!("movement step rejected: {error:?}"));
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

    fn quest_movement_has_live_target(&self, movement: &PendingMovement) -> bool {
        let quest = match &movement.work.key {
            QuestWorkKey::TravelToObjective { quest, .. }
            | QuestWorkKey::CollectItem { quest, .. } => *quest,
            _ => return false,
        };
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        matches!(
            resolve_objective(&snapshot, quest),
            ObjectiveResolution::GroundedCreature { .. }
                | ObjectiveResolution::GroundedGameObject { .. }
                | ObjectiveResolution::GroundedItemCreature { .. }
                | ObjectiveResolution::GroundedItemGameObject { .. }
                | ObjectiveResolution::GroundedControlledSpell { .. }
                | ObjectiveResolution::GroundedQuestSpell { .. }
                | ObjectiveResolution::GroundedScriptedItemUse { .. }
                | ObjectiveResolution::GroundedQuestTool { .. }
        )
    }

    async fn tick_quest(&mut self) -> bool {
        if self.pending_quest_action_blocks() {
            return true;
        }
        if let Some((target, completed_at, cached_target)) = self.post_combat_loot.clone() {
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
            if at.elapsed() < Duration::from_secs(10) {
                self.waiting(format!(
                    "awaiting server confirmation for quest {quest} from giver {giver}"
                ));
                return true;
            }
            tracing::warn!(lane=?self.state.lane, quest, ?giver, "quest accept confirmation timed out; allowing bounded retry");
            self.pending_accept = None;
        }
        if let Some((quest, at, step)) = self.pending_turn_in {
            if at.elapsed() < Duration::from_secs(10) {
                self.waiting(format!("awaiting authoritative server confirmation for quest {quest} turn-in step {step}"));
                return true;
            }
            tracing::warn!(lane=?self.state.lane, quest, step, "quest turn-in confirmation timed out; allowing bounded retry");
            self.pending_turn_in = None;
        }
        if let Some((giver, at)) = self.pending_giver_interaction {
            if at.elapsed() < Duration::from_secs(10) {
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
            .next()
            .map(|(&quest, offer)| (quest, offer.giver));
        if let Some((quest, giver)) = offer {
            self.set_work(QuestWorkKey::AcquireQuest { quest: Some(quest) });
            tracing::info!(lane=?self.state.lane, quest, ?giver, "quest scheduler accepting authoritative quest offer");
            self.pending_accept = Some((quest, giver, Instant::now()));
            return self
                .propose_command(GameplayCommand::AcceptQuest { quest, giver }, true)
                .await;
        }

        let incomplete_quest = self
            .state
            .authoritative
            .quests
            .active
            .iter()
            .find(|(_, progress)| !progress.complete)
            .map(|(&quest, _)| quest);
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
        let excluded: BTreeSet<(usize, EntityId)> = self
            .credited_quest_targets
            .iter()
            .filter_map(|(credited_quest, objective, target)| {
                (*credited_quest == quest).then_some((*objective, *target))
            })
            .collect();
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
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
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
                if self
                    .state
                    .authoritative
                    .position
                    .player
                    .is_some_and(|player| quest_tool_search_arrived(player.point, destination))
                {
                    self.waiting(format!("quest {quest} reached quest-control search area; waiting for live authoritative control object"));
                    return true;
                }
                tracing::info!(lane=?self.state.lane, quest, work_id=?work.id, x=destination.x, y=destination.y, "quest scheduler traveling to quest-bound control object search area");
                self.queue_movement(
                    destination,
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
                        GameplayCommand::UseGameObject(target),
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
                destination,
                alternatives,
                source,
            } => {
                let work = self.set_work(QuestWorkKey::TravelToObjective {
                    quest,
                    objective,
                    destination,
                });
                let key = (quest, objective, 0);
                let current_pos = self
                    .state
                    .authoritative
                    .position
                    .player
                    .map(|player| player.point);
                let arrived =
                    current_pos.is_some_and(|position| quest_search_arrived(position, destination));
                let candidates =
                    bounded_candidates(alternatives, current_pos.unwrap_or(destination));
                let Some(destination) =
                    self.next_search_destination(key, &candidates, arrived.then_some(destination))
                else {
                    self.waiting(format!("quest {quest} exhausted nearby {source} hints for objective {objective}; waiting before a bounded retry"));
                    return true;
                };
                tracing::info!(lane=?self.state.lane, quest, objective, work_id=?work.id, %source, x=destination.x, y=destination.y, "quest scheduler starting bounded objective-area travel");
                self.queue_search_movement(destination, work);
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
                let current_pos = self
                    .state
                    .authoritative
                    .position
                    .player
                    .map(|player| player.point);
                let candidates = bounded_candidates(destinations, current_pos.unwrap_or_default());
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
                    self.waiting(format!("quest {quest} exhausted nearby loot-source hints for item {item} at {current}/{required}; waiting before a bounded retry"));
                    return true;
                };
                if reached_destination != Some(destination) {
                    tracing::info!(lane=?self.state.lane, quest, item, current, required, work_id=?work.id, x=destination.x, y=destination.y, "quest item objective using AzerothCore loot-source search hint");
                    self.queue_search_movement(destination, work);
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
                wow_state::quests::QuestTurnInStage::OfferReward { reward_choices } => {
                    tracing::info!(lane=?self.state.lane, quest, giver=?dialog.giver, reward_choices, "quest turn-in choosing first authoritative reward option");
                    self.pending_turn_in = Some((quest, Instant::now(), "choose-reward"));
                    return self
                        .propose_command(
                            GameplayCommand::ChooseQuestReward {
                                quest,
                                giver: dialog.giver,
                                reward: 0,
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
        if let Some(player) = self.state.authoritative.position.player {
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
                tracing::info!(lane=?self.state.lane, quest, work_id=?work.id, x=destination.x, y=destination.y, "completed quest using AzerothCore turn-in search hint");
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

    fn record_search_arrival(&mut self, movement: &PendingMovement) {
        if movement.purpose != MovementPurpose::SearchArea {
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
            .insert(search_point_key(movement.destination));
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
            .insert(key, now + Duration::from_secs(30));
        None
    }

    fn pending_quest_action_blocks(&mut self) -> bool {
        let Some(pending) = self.pending_quest_action.clone() else {
            return false;
        };
        match pending {
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
                if generation > baseline_generation
                    && self.state.authoritative.inventory.current_loot.is_none()
                {
                    tracing::info!(lane=?self.state.lane, ?target, baseline_generation, generation, "authoritative bot-owned post-combat loot transaction completed");
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
                let transaction_completed = generation > baseline_generation
                    && self.state.authoritative.inventory.current_loot.is_none();
                let gone = !self.state.authoritative.entities.0.contains_key(&target);
                if player_superseded {
                    tracing::info!(lane=?self.state.lane, item, ?target, "pending bot loot was superseded by player loot; cancelling without bot-success credit");
                    self.pending_quest_action = None;
                    return false;
                }
                if current > baseline_count || gone || transaction_completed {
                    tracing::info!(lane=?self.state.lane, item, ?target, baseline_count, current, baseline_generation, generation, transaction_completed, gone, "authoritative loot progress observed");
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

    async fn dispatch_combat_target(&mut self, target: EntityId, recovery: bool) -> bool {
        let snapshot = Snapshot::from_state(&self.state.authoritative);
        let selected = match wow_policy::combat::selector::select_action(&snapshot, target) {
            Ok(selected) => selected,
            Err(reason) => {
                let source = if recovery { "survival" } else { "quest" };
                self.waiting(format!(
                    "{source} combat against {target} deferred: {reason}"
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
            _ => {}
        }
        true
    }

    async fn propose_command(&mut self, command: GameplayCommand, quest_step: bool) -> bool {
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
        self.submit(action).await
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
                destination,
            })
        });
        tracing::info!(lane=?self.state.lane, work_id=?work.id, ?destination, acceptable_range, resume=?resume, %label, "shared spatial precondition handed off to owned movement");
        let purpose = if origin == PlanOrigin::Recovery {
            MovementPurpose::SurvivalApproach
        } else {
            MovementPurpose::ApproachGroundedTarget
        };
        self.queue_movement(
            destination,
            acceptable_range,
            Some(resume),
            None,
            origin,
            work,
            purpose,
        );
        self.last_dispatch = DispatchOutcome::DeferredMovement;
    }

    async fn submit(&mut self, action: ProposedAction) -> bool {
        if !self.state.runnable() {
            self.last_dispatch = DispatchOutcome::Rejected;
            return true;
        }
        let original_command = action.command.clone();
        let action_origin = action.origin;
        let mut snapshot = Snapshot::from_state(&self.state.authoritative);
        if let GameplayCommand::Loot(target) = &original_command
            && let Some((post_target, _, Some(cached_target))) = &self.post_combat_loot
            && post_target == target
        {
            snapshot
                .state
                .entities
                .0
                .entry(*target)
                .or_insert_with(|| cached_target.clone());
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
            action,
        ) {
            ValidationOutcome::Sendable(action) => {
                tracing::info!(lane=?self.state.lane, action=?action.id(), command=?action.command(), "validated gameplay action queued for proxy");
                let sent = self.proxy.send(WorkerToProxy::Action(action)).await.is_ok();
                self.last_dispatch = if sent {
                    DispatchOutcome::Sent
                } else {
                    DispatchOutcome::TransportClosed
                };
                sent
            }
            ValidationOutcome::NeedsMovement(requirement) => {
                self.defer_spatial_movement(
                    requirement.destination,
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
            face,
        ) {
            ValidationOutcome::Sendable(face) => {
                tracing::info!(lane=?self.state.lane, orientation=requirement.orientation, tolerance=requirement.tolerance, resume=?original_command, "shared facing precondition queued before targeted action");
                let sent = self.proxy.send(WorkerToProxy::Action(face)).await.is_ok();
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
        self.last_quest_step = None;
        self.pending_accept = None;
        self.pending_giver_interaction = None;
        self.giver_retry_after.clear();
        self.pending_turn_in = None;
        self.last_turn_in_search_query = None;
        self.pending_movement = None;
        self.pending_quest_action = None;
        self.interaction_retry_after.clear();
        self.search_attempts.clear();
        self.search_retry_after.clear();
        self.current_work = None;
        self.last_wait_reason = None;
        self.last_dispatch = DispatchOutcome::Rejected;
        self.credited_quest_targets.clear();
        self.los_blocked.clear();
        self.los_attempts.clear();
        self.server_range_recovery.clear();
        self.behind_reposition_pending.clear();
        self.behind_retry_after.clear();
        self.behind_retry_cast_allowed.clear();
        self.maintenance_retry_after.clear();
        self.last_maintenance_tick = None;
        self.last_maintenance_status = None;
        self.post_combat_loot = None;
        self.last_recovery_action = None;
        self.last_survival_action = None;
    }
}

fn quest_status_available(status: u8) -> bool {
    matches!(status, 2 | 4 | 7 | 8)
}
fn quest_status_reward(status: u8) -> bool {
    matches!(status, 3 | 6 | 9 | 10)
}
fn giver_retry_delay(attempts: u8) -> Duration {
    Duration::from_secs(
        5_u64
            .saturating_mul(1_u64 << attempts.saturating_sub(1).min(3))
            .min(40),
    )
}

fn bounded_candidates(mut candidates: Vec<Vec3>, origin: Vec3) -> Vec<Vec3> {
    candidates.retain(|point| point.is_finite() && point.distance(origin) <= 250.0);
    candidates.sort_by(|a, b| origin.distance(*a).total_cmp(&origin.distance(*b)));
    candidates.dedup();
    candidates.truncate(5);
    candidates
}

fn search_point_key(point: Vec3) -> (u32, u32, u32) {
    (point.x.to_bits(), point.y.to_bits(), point.z.to_bits())
}
fn turn_in_search_arrived(player: Vec3, destination: Vec3) -> bool {
    (destination.x - player.x).hypot(destination.y - player.y) <= TURN_IN_SEARCH_RANGE
}
fn quest_search_arrived(player: Vec3, destination: Vec3) -> bool {
    player.distance(destination) <= QUEST_SEARCH_ARRIVAL_RANGE
}
fn quest_tool_search_arrived(player: Vec3, destination: Vec3) -> bool {
    player.distance(destination) <= QUEST_TOOL_SEARCH_RANGE
}
fn should_supersede_search_movement(purpose: MovementPurpose, live_target_available: bool) -> bool {
    purpose == MovementPurpose::SearchArea && live_target_available
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ActivityArbiter;

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
                destination,
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

        assert!(
            engine
                .handle(LaneMessage::Observation(ProtocolObservation::LeftWorld))
                .await
        );
        assert!(engine.pending_movement.is_none());
        assert!(engine.pending_quest_action.is_none());
        assert!(engine.pending_accept.is_none());
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
    fn live_target_only_supersedes_static_search_movement() {
        assert!(should_supersede_search_movement(
            MovementPurpose::SearchArea,
            true
        ));
        assert!(!should_supersede_search_movement(
            MovementPurpose::ApproachGroundedTarget,
            true
        ));
        assert!(!should_supersede_search_movement(
            MovementPurpose::SearchArea,
            false
        ));
    }

    #[test]
    fn turn_in_search_continues_past_the_old_eighteen_yard_envelope() {
        let destination = Vec3::new(0.0, 0.0, 20.0);
        assert!(!turn_in_search_arrived(
            Vec3::new(17.8, 0.0, 0.0),
            destination
        ));
        assert!(turn_in_search_arrived(
            Vec3::new(4.9, 0.0, 0.0),
            destination
        ));
    }

    #[test]
    fn static_search_advances_after_arrival_and_waits_after_bounded_candidates() {
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
        assert!(engine.search_retry_after[&key] > Instant::now());
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
