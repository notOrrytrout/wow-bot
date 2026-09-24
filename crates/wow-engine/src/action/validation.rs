use super::spatial;
use wow_domain::*;
use wow_state::Snapshot;

pub struct ValidationContext {
    pub current: ValidityStamp,
    pub stage: ActivationStage,
    pub permissions: PermissionSet,
}

pub struct ActionValidator;

impl ActionValidator {
    pub fn validate(
        snapshot: &Snapshot,
        context: ValidationContext,
        action: ProposedAction,
    ) -> ValidationOutcome {
        let current = context.current;
        if snapshot.revision != current.state || action.stamp.state != current.state {
            return reject("stale_state", "state revision changed", true);
        }
        if action.stamp.mission != current.mission {
            return reject("stale_mission", "mission revision changed", false);
        }
        if action.stamp.permission != current.permission {
            return reject("stale_permission", "permission revision changed", false);
        }
        if action.stamp.worker != current.worker {
            return reject("stale_worker", "worker generation changed", false);
        }
        if action.stamp.ownership != current.ownership {
            return reject("stale_ownership", "ownership generation changed", false);
        }
        if snapshot.state.desync.suspect {
            return reject(
                "desynchronized",
                "required authoritative state is desynchronized",
                true,
            );
        }
        if !snapshot.state.session.in_world {
            return reject("not_in_world", "character is not in world", true);
        }
        if !origin_authorized(action.origin, &action.command) {
            return reject("origin_forbidden", "plan origin lacks authority", false);
        }
        if !stage_allows(context.stage, &action.command) {
            return reject(
                "activation_stage",
                "action is not enabled at the current activation stage",
                true,
            );
        }
        let required = required_permission(&action.command);
        if action.origin != PlanOrigin::Recovery
            && !required.is_empty()
            && !context.permissions.contains(required)
        {
            return reject(
                "permission_denied",
                "mission permissions do not authorize the action",
                false,
            );
        }

        if let Some(movement) = spatial::movement_requirement(snapshot, &action.command) {
            return ValidationOutcome::NeedsMovement(movement);
        }
        if let Some(facing) = spatial::facing_requirement(snapshot, &action.command) {
            return ValidationOutcome::NeedsFacing(facing);
        }

        if let Err(outcome) = validate_command(snapshot, &action.command) {
            return outcome;
        }
        send(action)
    }
}

fn validate_command(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Result<(), ValidationOutcome> {
    validate_spatial_command(snapshot, command)?;
    validate_quest_command(snapshot, command)?;
    validate_economy_command(snapshot, command)
}

fn validate_spatial_command(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Result<(), ValidationOutcome> {
    match command {
        GameplayCommand::MoveTo(point) if !point.is_finite() => {
            return Err(reject(
                "bad_position",
                "movement destination is invalid",
                false,
            ));
        }
        GameplayCommand::FaceDirection { orientation } if !orientation.is_finite() => {
            return Err(reject(
                "bad_orientation",
                "facing orientation is invalid",
                false,
            ));
        }
        GameplayCommand::Attack(entity)
            if !snapshot.state.entities.0.get(entity).is_some_and(|e| {
                e.hostile
                    || active_quest_authorizes_attack(snapshot, e.entry)
                    || wow_policy::combat::engagement::is_attacking_player_or_group(
                        snapshot, *entity,
                    )
            }) =>
        {
            return Err(reject(
                "combat_authority",
                "target is not an authoritative hostile, active quest target, or engaged survival attacker",
                false,
            ));
        }
        GameplayCommand::Interact(entity)
        | GameplayCommand::UseGameObject(entity)
        | GameplayCommand::CastGameObject { target: entity, .. }
        | GameplayCommand::Loot(entity)
        | GameplayCommand::Gather(entity)
        | GameplayCommand::EnterVehicle(entity) => {
            require_authoritative_entity(
                snapshot,
                Some(*entity),
                "unknown_entity",
                "target is not authoritative",
            )?;
        }
        GameplayCommand::Cast { spell, target } => {
            if !snapshot.state.capabilities.spells.contains(spell)
                && !(*spell == 5019 && wow_policy::combat::selector::has_equipped_wand(snapshot))
            {
                return Err(reject(
                    "unknown_spell",
                    "spell is not an authoritative known capability or equipped-wand Shoot",
                    false,
                ));
            }
            require_authoritative_entity(
                snapshot,
                *target,
                "unknown_entity",
                "spell target is not authoritative",
            )?;
        }
        GameplayCommand::MaintainBuff { spell, target } => {
            if !snapshot.state.capabilities.spells.contains(spell) {
                return Err(reject(
                    "unknown_spell",
                    "maintenance spell is not an authoritative known capability",
                    false,
                ));
            }
            let self_guid = snapshot.state.session.character_guid.map(EntityId);
            if Some(*target) != self_guid {
                require_authoritative_entity(
                    snapshot,
                    Some(*target),
                    "unknown_entity",
                    "maintenance target is not authoritative",
                )?;
            }
        }
        GameplayCommand::Fish if !snapshot.state.capabilities.can_fish => {
            return Err(reject(
                "cannot_fish",
                "fishing capability is not authoritative",
                false,
            ));
        }
        GameplayCommand::UseItem { item, target } => {
            if !snapshot.state.inventory.has(*item, 1) {
                return Err(reject("missing_item", "item is no longer present", false));
            }
            require_authoritative_entity(
                snapshot,
                *target,
                "unknown_entity",
                "item target is not authoritative",
            )?;
        }
        GameplayCommand::UseItemInstance {
            item,
            item_guid,
            backpack_slot,
            target,
            ..
        } => {
            let matches = snapshot
                .state
                .inventory
                .instances
                .get(item_guid)
                .is_some_and(|instance| {
                    instance.item == *item
                        && instance.backpack_slot == *backpack_slot
                        && instance.count > 0
                });
            if !matches {
                return Err(reject(
                    "stale_item_instance",
                    "item slot/GUID is no longer authoritative",
                    true,
                ));
            }
            require_authoritative_entity(
                snapshot,
                *target,
                "unknown_entity",
                "item target is not authoritative",
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_quest_command(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Result<(), ValidationOutcome> {
    match command {
        GameplayCommand::AcceptQuest { quest, giver } => {
            require_authoritative_entity(
                snapshot,
                Some(*giver),
                "unknown_quest_giver",
                "quest giver is not authoritative",
            )?;
            if snapshot.state.quests.active.contains_key(quest)
                || snapshot.state.quests.completed.contains(quest)
            {
                return Err(reject(
                    "quest_not_available",
                    "quest is already active or complete",
                    false,
                ));
            }
        }
        GameplayCommand::TurnInQuest { quest, giver }
        | GameplayCommand::RequestQuestReward { quest, giver }
        | GameplayCommand::ChooseQuestReward { quest, giver, .. } => {
            require_authoritative_entity(
                snapshot,
                Some(*giver),
                "unknown_quest_giver",
                "quest giver is not authoritative",
            )?;
            if !snapshot
                .state
                .quests
                .active
                .get(quest)
                .is_some_and(|q| q.complete)
            {
                return Err(reject(
                    "quest_not_complete",
                    "authoritative quest state is not complete",
                    true,
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_economy_command(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Result<(), ValidationOutcome> {
    match command {
        GameplayCommand::VendorBuy { vendor, .. } | GameplayCommand::VendorSell { vendor, .. }
            if snapshot.state.inventory.vendor != Some(*vendor) =>
        {
            return Err(reject(
                "vendor_not_open",
                "vendor interaction is not currently authoritative",
                true,
            ));
        }
        GameplayCommand::VendorSell { item, count, .. }
            if !snapshot.state.inventory.has(*item, *count) =>
        {
            return Err(reject(
                "missing_item",
                "sale item quantity is no longer present",
                false,
            ));
        }
        GameplayCommand::TradeAccept { generation, .. }
            if snapshot.state.inventory.trade.generation != *generation
                || !snapshot.state.inventory.trade.open =>
        {
            return Err(reject("stale_trade", "trade generation changed", true));
        }
        GameplayCommand::TradeAccept {
            gift_only: true, ..
        } if !snapshot.state.inventory.trade.our_items.is_empty()
            || snapshot.state.inventory.trade.our_money != 0 =>
        {
            return Err(reject(
                "not_clear_gift",
                "gift acceptance would send reciprocal assets",
                false,
            ));
        }
        GameplayCommand::AuctionBuy {
            query_generation,
            listing_id,
            max_buyout,
        } => {
            if snapshot.state.inventory.auction.query_generation != *query_generation {
                return Err(reject(
                    "stale_auction",
                    "auction query generation changed",
                    true,
                ));
            }
            let Some(listing) = snapshot.state.inventory.auction.listings.get(listing_id) else {
                return Err(reject(
                    "listing_missing",
                    "auction listing is no longer current",
                    true,
                ));
            };
            if listing.buyout > *max_buyout || listing.buyout > snapshot.state.inventory.money {
                return Err(reject(
                    "auction_price",
                    "listing exceeds authorized buyout or current funds",
                    false,
                ));
            }
        }
        GameplayCommand::MailTake {
            mailbox_generation,
            mail_id,
        } => {
            if snapshot.state.inventory.mailbox.generation != *mailbox_generation
                || !snapshot.state.inventory.mailbox.mails.contains_key(mail_id)
            {
                return Err(reject("stale_mail", "mailbox contents changed", true));
            }
        }
        _ => {}
    }
    Ok(())
}

fn require_authoritative_entity(
    snapshot: &Snapshot,
    target: Option<EntityId>,
    code: &str,
    message: &str,
) -> Result<(), ValidationOutcome> {
    if target.is_none_or(|entity| has_entity(snapshot, entity)) {
        Ok(())
    } else {
        Err(reject(code, message, true))
    }
}

fn has_entity(snapshot: &Snapshot, entity: EntityId) -> bool {
    snapshot.state.entities.0.contains_key(&entity)
}

fn send(action: ProposedAction) -> ValidationOutcome {
    ValidationOutcome::Sendable(SendableAction::from_validated(ValidatedAction {
        id: action.id,
        task: action.task,
        origin: action.origin,
        stamp: action.stamp,
        command: action.command,
    }))
}

fn required_permission(command: &GameplayCommand) -> PermissionSet {
    match command {
        GameplayCommand::MoveTo(_)
        | GameplayCommand::FaceDirection { .. }
        | GameplayCommand::StopMovement => PermissionSet::MOVE,
        GameplayCommand::ReleaseSpirit
        | GameplayCommand::QueryCorpse
        | GameplayCommand::ReclaimCorpse { .. } => PermissionSet::empty(),
        GameplayCommand::Attack(_)
        | GameplayCommand::Cast { .. }
        | GameplayCommand::EnterVehicle(_)
        | GameplayCommand::VehicleCast { .. } => PermissionSet::COMBAT,
        GameplayCommand::MaintainBuff { .. } => PermissionSet::MAINTENANCE,
        GameplayCommand::Loot(_) => PermissionSet::LOOT,
        GameplayCommand::Gather(_) | GameplayCommand::Fish => PermissionSet::GATHER,
        GameplayCommand::QueryQuestGivers
        | GameplayCommand::QueryQuest { .. }
        | GameplayCommand::AcceptQuest { .. }
        | GameplayCommand::TurnInQuest { .. }
        | GameplayCommand::RequestQuestReward { .. }
        | GameplayCommand::ChooseQuestReward { .. } => PermissionSet::QUEST,
        GameplayCommand::VendorBuy { .. } | GameplayCommand::VendorSell { .. } => {
            PermissionSet::ECONOMY
        }
        GameplayCommand::TradeAccept { .. }
        | GameplayCommand::AuctionBuy { .. }
        | GameplayCommand::MailTake { .. } => {
            PermissionSet::ECONOMY | PermissionSet::ASSET_TRANSFER
        }
        GameplayCommand::Chat { .. } => PermissionSet::CHAT,
        GameplayCommand::Raw { .. } => PermissionSet::SERVER_COMMAND,
        GameplayCommand::Interact(_)
        | GameplayCommand::UseGameObject(_)
        | GameplayCommand::CastGameObject { .. }
        | GameplayCommand::UseItem { .. }
        | GameplayCommand::UseItemInstance { .. } => PermissionSet::empty(),
    }
}

fn origin_authorized(origin: PlanOrigin, command: &GameplayCommand) -> bool {
    origin.permits(command)
}

fn stage_allows(stage: ActivationStage, command: &GameplayCommand) -> bool {
    match stage {
        ActivationStage::Observe => matches!(command, GameplayCommand::Chat { .. }),
        ActivationStage::Maintain => matches!(
            command,
            GameplayCommand::Chat { .. }
                | GameplayCommand::UseItem { .. }
                | GameplayCommand::UseItemInstance { .. }
                | GameplayCommand::MaintainBuff { .. }
        ),
        ActivationStage::Move => matches!(
            command,
            GameplayCommand::Chat { .. }
                | GameplayCommand::UseItem { .. }
                | GameplayCommand::UseItemInstance { .. }
                | GameplayCommand::MaintainBuff { .. }
                | GameplayCommand::MoveTo(_)
                | GameplayCommand::FaceDirection { .. }
                | GameplayCommand::StopMovement
        ),
        ActivationStage::Act => true,
    }
}

fn active_quest_authorizes_attack(snapshot: &Snapshot, entry: u32) -> bool {
    active_quest_definitions(snapshot).any(|(progress, definition)| {
        let incomplete_creature_target = definition.targets.iter().any(|target| {
            target.kind == wow_state::quests::QuestTargetKind::Creature
                && target.entry == entry
                && progress
                    .objectives
                    .get(target.slot)
                    .copied()
                    .unwrap_or_default()
                    < target.required
        });
        let unmet_creature_item_source = definition.items.iter().any(|item| {
            let current = snapshot.state.inventory.count(item.item);
            current < item.required
                && wow_policy::questing::static_hints::item_source_entries(item.item)
                    .iter()
                    .any(|(kind, source_entry)| {
                        *kind == wow_state::quests::QuestTargetKind::Creature
                            && *source_entry == entry
                    })
        });
        incomplete_creature_target || unmet_creature_item_source
    })
}

fn active_quest_definitions(
    snapshot: &Snapshot,
) -> impl Iterator<
    Item = (
        &wow_state::quests::QuestProgress,
        &wow_state::quests::QuestDefinition,
    ),
> {
    snapshot
        .state
        .quests
        .active
        .iter()
        .filter(|(_, progress)| !progress.complete)
        .filter_map(|(quest, progress)| {
            snapshot
                .state
                .quests
                .definitions
                .get(quest)
                .map(|definition| (progress, definition))
        })
}

fn reject(code: &str, message: &str, retryable: bool) -> ValidationOutcome {
    ValidationOutcome::Rejected(ActionFailure {
        code: code.into(),
        message: message.into(),
        retryable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::AuthoritativeState;

    fn base_stamp() -> ValidityStamp {
        ValidityStamp {
            state: StateRevision(7),
            mission: MissionRevision(3),
            permission: PermissionRevision(2),
            worker: WorkerGeneration(4),
            ownership: OwnershipGeneration(5),
            movement: MovementEpoch(6),
        }
    }

    fn snapshot() -> Snapshot {
        let mut state = AuthoritativeState::default();
        state.revision = StateRevision(7);
        state.session.in_world = true;
        Snapshot::from_state(&state)
    }

    #[test]
    fn stale_state_is_rejected_before_sendable_creation() {
        let mut stamp = base_stamp();
        stamp.state = StateRevision(6);
        let action = ProposedAction {
            id: ActionId(1),
            task: TaskId(1),
            origin: PlanOrigin::Operator,
            stamp,
            command: GameplayCommand::StopMovement,
        };
        let outcome = ActionValidator::validate(
            &snapshot(),
            ValidationContext {
                current: base_stamp(),
                stage: ActivationStage::Act,
                permissions: PermissionSet::ALL,
            },
            action,
        );
        assert!(
            matches!(outcome, ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "stale_state")
        );
    }

    #[test]
    fn item_instance_validation_accepts_each_observed_stack() {
        use wow_state::inventory::InventoryItemInstance;
        let mut state = AuthoritativeState::default();
        state.revision = base_stamp().state;
        state.session.in_world = true;
        for (guid, slot) in [(10, 3), (11, 1)] {
            state.inventory.instances.insert(
                EntityId(guid),
                InventoryItemInstance {
                    item: 99,
                    guid: EntityId(guid),
                    backpack_slot: slot,
                    count: 1,
                },
            );
        }
        let snapshot = Snapshot::from_state(&state);
        let validate = |guid, slot| {
            ActionValidator::validate(
                &snapshot,
                ValidationContext {
                    current: base_stamp(),
                    stage: ActivationStage::Act,
                    permissions: PermissionSet::ALL,
                },
                ProposedAction {
                    id: ActionId(1),
                    task: TaskId(1),
                    origin: PlanOrigin::Operator,
                    stamp: base_stamp(),
                    command: GameplayCommand::UseItemInstance {
                        item: 99,
                        item_guid: EntityId(guid),
                        backpack_slot: slot,
                        spell: 0,
                        target: None,
                        cast_count: 0,
                    },
                },
            )
        };
        assert!(matches!(validate(10, 3), ValidationOutcome::Sendable(_)));
        assert!(matches!(validate(11, 1), ValidationOutcome::Sendable(_)));
        assert!(
            matches!(validate(12, 1), ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "stale_item_instance")
        );
    }

    #[test]
    fn recovery_attack_is_authorized_for_npc_targeting_player_without_pull_ownership() {
        use wow_state::entities::{EntityKind, EntityState};
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                kind: EntityKind::Unit,
                health: Some((10, 10)),
                target: Some(EntityId(1)),
                ..Default::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        let stamp = ValidityStamp::default();
        let action = ProposedAction {
            id: ActionId(1),
            task: TaskId(1),
            origin: PlanOrigin::Recovery,
            stamp,
            command: GameplayCommand::Attack(EntityId(9)),
        };
        let result = ActionValidator::validate(
            &snapshot,
            ValidationContext {
                current: stamp,
                stage: ActivationStage::Act,
                permissions: PermissionSet::empty(),
            },
            action,
        );
        assert!(matches!(result, ValidationOutcome::Sendable(_)));
    }

    #[test]
    fn controlled_vehicle_cast_requests_movement_from_controlled_mover() {
        use wow_state::entities::{EntityKind, EntityState};
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.position.player = Some(WorldPosition {
            map: 609,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.control.mover = Some(EntityId(500));
        state.control.mover_position = Some(WorldPosition {
            map: 609,
            point: Vec3::new(0.0, 0.0, 40.0),
            orientation: 0.0,
        });
        state.entities.0.insert(
            EntityId(900),
            EntityState {
                id: EntityId(900),
                entry: 28525,
                kind: EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 609,
                    point: Vec3::new(50.0, 0.0, 40.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        let requirement = spatial::movement_requirement(
            &snapshot,
            &GameplayCommand::VehicleCast {
                spell: 51858,
                target: Some(EntityId(900)),
            },
        )
        .expect("out-of-range controlled spell should request movement");
        assert_eq!(requirement.destination, Vec3::new(50.0, 0.0, 40.0));
        assert_eq!(requirement.acceptable_range, 18.0);
    }

    #[test]
    fn dialogue_cannot_escape_chat_authority() {
        let action = ProposedAction {
            id: ActionId(2),
            task: TaskId(2),
            origin: PlanOrigin::Dialogue,
            stamp: base_stamp(),
            command: GameplayCommand::Raw {
                opcode: 1,
                body: vec![],
            },
        };
        let outcome = ActionValidator::validate(
            &snapshot(),
            ValidationContext {
                current: base_stamp(),
                stage: ActivationStage::Act,
                permissions: PermissionSet::ALL,
            },
            action,
        );
        assert!(
            matches!(outcome, ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "origin_forbidden")
        );
    }
}
