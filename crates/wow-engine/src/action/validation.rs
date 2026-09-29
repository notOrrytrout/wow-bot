use super::spatial;
use wow_domain::time::Millis;
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
        GameplayCommand::Attack(entity) if !attack_authorized(snapshot, *entity) => {
            return Err(reject(
                "combat_authority",
                "target is not an authoritative hostile, active quest target, or engaged survival attacker",
                false,
            ));
        }
        GameplayCommand::PetAttack { pet, target } => {
            if snapshot.state.pet.guid != Some(*pet)
                || snapshot
                    .state
                    .entities
                    .0
                    .get(pet)
                    .is_some_and(|entity| entity.is_dead())
                || !has_entity(snapshot, *target)
            {
                return Err(reject(
                    "stale_pet_attack",
                    "pet or target state is not authoritative",
                    true,
                ));
            }
            if !attack_authorized(snapshot, *target) {
                return Err(reject(
                    "combat_authority",
                    "target is not authorized for pet attack",
                    false,
                ));
            }
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
        GameplayCommand::CastOnItem { spell, item_guid } => {
            if snapshot.state.capabilities.class_id != Some(7)
                || !snapshot.state.capabilities.spells.contains(spell)
            {
                return Err(reject(
                    "invalid_item_cast",
                    "item-targeted imbue is not a known Shaman spell",
                    false,
                ));
            }
            if !snapshot
                .state
                .inventory
                .equipped_item_instances
                .iter()
                .any(|(slot, item)| {
                    matches!(*slot, 15 | 16)
                        && item.guid == *item_guid
                        && item.temporary_enchanted == Some(false)
                })
            {
                return Err(reject(
                    "invalid_item_target",
                    "target is not an observed unenchanted main-hand or off-hand weapon",
                    true,
                ));
            }
            if let Err(reason) = wow_policy::combat::readiness::check_spell_readiness(
                snapshot,
                *spell,
                snapshot.state.session.character_guid.map(EntityId),
                Millis::wall_clock_now().0,
            ) {
                return Err(reject(
                    "spell_unavailable",
                    &format!("weapon imbue is not ready: {reason:?}"),
                    true,
                ));
            }
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
        GameplayCommand::SummonPet { spell, player } => {
            if snapshot.state.capabilities.class_id != Some(9)
                || !wow_policy::maintenance::is_warlock_persistent_demon_summon(*spell)
            {
                return Err(reject(
                    "invalid_pet_summon",
                    "pet summon is not an allowed persistent Warlock demon spell",
                    false,
                ));
            }
            if snapshot.state.session.character_guid != Some(player.0) {
                return Err(reject(
                    "invalid_pet_summon_target",
                    "pet summon correlation target must be the current character",
                    false,
                ));
            }
            if !snapshot.state.capabilities.spells.contains(spell) {
                return Err(reject(
                    "unknown_spell",
                    "pet summon is not an authoritative known capability",
                    false,
                ));
            }
            if let Err(reason) = wow_policy::combat::readiness::check_spell_readiness(
                snapshot,
                *spell,
                None,
                Millis::wall_clock_now().0,
            ) {
                return Err(reject(
                    "spell_unavailable",
                    &format!("pet summon is not ready: {reason:?}"),
                    true,
                ));
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
        GameplayCommand::UseItemOnItem {
            item,
            item_guid,
            backpack_slot,
            spell,
            target_item_guid,
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
            let poison_template = snapshot
                .state
                .inventory
                .item_metadata
                .get(item)
                .is_some_and(|metadata| {
                    metadata.item_class == 0
                        && metadata.subclass == 6
                        && metadata.use_spell_id == *spell
                });
            let valid_target =
                snapshot
                    .state
                    .inventory
                    .equipped_item_instances
                    .iter()
                    .any(|(slot, equipped)| {
                        matches!(*slot, 15 | 16)
                            && equipped.guid == *target_item_guid
                            && equipped.temporary_enchanted == Some(false)
                    });
            if snapshot.state.capabilities.class_id != Some(4)
                || !matches
                || !poison_template
                || !valid_target
            {
                return Err(reject(
                    "invalid_poison_application",
                    "poison and weapon instances must match authoritative inventory state",
                    true,
                ));
            }
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
        GameplayCommand::EquipItem {
            item_guid,
            destination_slot,
        } => {
            let Some(instance) = snapshot.state.inventory.instances.get(item_guid) else {
                return Err(reject(
                    "stale_equipment",
                    "equipment or item instance is not authoritative",
                    true,
                ));
            };
            let Some(metadata) = snapshot.state.inventory.item_metadata.get(&instance.item) else {
                return Err(reject(
                    "missing_item_metadata",
                    "item metadata is not authoritative",
                    true,
                ));
            };
            let player_level = snapshot
                .state
                .session
                .character_guid
                .and_then(|guid| snapshot.state.entities.0.get(&EntityId(guid)))
                .and_then(|player| player.level)
                .unwrap_or_default()
                .min(255) as u8;
            let Some(class) = snapshot.state.capabilities.class_id else {
                return Err(reject(
                    "unknown_player_class",
                    "player class is not authoritative",
                    true,
                ));
            };
            let bag_candidate = (19..=22).contains(destination_slot)
                && wow_policy::gear::container_can_equip(metadata, class, player_level);
            let gear_candidate = wow_policy::gear::player_can_use(metadata, class, player_level)
                && wow_policy::gear::destination_slots(metadata).contains(destination_slot);
            if !snapshot.state.inventory.equipment_slots_authoritative
                || instance.backpack_slot < 23
                || (!bag_candidate && !gear_candidate)
            {
                return Err(reject(
                    "invalid_equipment_target",
                    "item cannot equip to the selected authoritative slot",
                    false,
                ));
            }
        }
        GameplayCommand::PetSetReaction { pet, reaction } => {
            if snapshot.state.pet.guid != Some(*pet)
                || *reaction > 2
                || snapshot
                    .state
                    .entities
                    .0
                    .get(pet)
                    .is_some_and(|entity| entity.is_dead())
            {
                return Err(reject("stale_pet", "pet control state changed", true));
            }
        }
        GameplayCommand::PetSetAutocast { pet, spell, .. } => {
            if snapshot.state.pet.guid != Some(*pet)
                || snapshot
                    .state
                    .entities
                    .0
                    .get(pet)
                    .is_some_and(|entity| entity.is_dead())
                || !snapshot
                    .state
                    .pet
                    .abilities
                    .iter()
                    .any(|ability| ability.spell == *spell && ability.autocast.is_some())
            {
                return Err(reject(
                    "stale_pet_ability",
                    "pet ability is not authoritatively autocastable",
                    true,
                ));
            }
        }
        GameplayCommand::VendorBuy { vendor, .. } | GameplayCommand::VendorSell { vendor, .. }
            if snapshot.state.inventory.vendor != Some(*vendor) =>
        {
            return Err(reject(
                "vendor_not_open",
                "vendor interaction is not currently authoritative",
                true,
            ));
        }
        GameplayCommand::VendorSell {
            item,
            item_guid,
            count,
            ..
        } if !snapshot
            .state
            .inventory
            .instances
            .get(item_guid)
            .is_some_and(|instance| instance.item == *item && instance.count >= *count) =>
        {
            return Err(reject(
                "missing_item",
                "sale item instance or quantity is no longer present",
                false,
            ));
        }
        GameplayCommand::RepairEquipment { vendor } => {
            let Some(entity) = snapshot.state.entities.0.get(vendor) else {
                return Err(reject(
                    "unknown_repair_vendor",
                    "repair vendor is not currently observed",
                    true,
                ));
            };
            let is_repair_vendor = entity.kind == wow_state::entities::EntityKind::Unit
                && entity.interactable
                && wow_infra::world_knowledge::embedded_azerothcore_catalog()
                    .world()
                    .vendor_services
                    .iter()
                    .any(|service| service.entry_id == entity.entry && service.can_repair);
            if !is_repair_vendor {
                return Err(reject(
                    "not_a_repair_vendor",
                    "current entity data and trusted service catalog do not establish repair service",
                    false,
                ));
            }
            let condition = snapshot.state.inventory.equipment_condition;
            if !condition.observed {
                return Err(reject(
                    "unknown_equipment_condition",
                    "current equipment durability is not authoritative",
                    true,
                ));
            }
            if !wow_policy::maintenance::equipment_needs_repair(condition) {
                return Err(reject(
                    "repair_not_needed",
                    "authoritative equipment durability does not require repair",
                    false,
                ));
            }
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

fn attack_authorized(snapshot: &Snapshot, target: EntityId) -> bool {
    snapshot
        .state
        .entities
        .0
        .get(&target)
        .is_some_and(|entity| {
            entity.hostile
                || active_quest_authorizes_attack(snapshot, entity.entry)
                || wow_policy::combat::engagement::is_attacking_player_or_group(snapshot, target)
        })
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
        | GameplayCommand::PetAttack { .. }
        | GameplayCommand::Cast { .. }
        | GameplayCommand::EnterVehicle(_)
        | GameplayCommand::VehicleCast { .. } => PermissionSet::COMBAT,
        GameplayCommand::MaintainBuff { .. } | GameplayCommand::SummonPet { .. } => {
            PermissionSet::MAINTENANCE
        }
        GameplayCommand::CastOnItem { .. } | GameplayCommand::UseItemOnItem { .. } => {
            PermissionSet::MAINTENANCE
        }
        GameplayCommand::EquipItem { .. } => PermissionSet::MAINTENANCE,
        GameplayCommand::RepairEquipment { .. } => PermissionSet::MAINTENANCE,
        GameplayCommand::PetSetReaction { .. } | GameplayCommand::PetSetAutocast { .. } => {
            PermissionSet::MAINTENANCE
        }
        GameplayCommand::Loot(_) => PermissionSet::LOOT,
        GameplayCommand::Gather(_) | GameplayCommand::Fish => PermissionSet::GATHER,
        GameplayCommand::QueryQuestGivers
        | GameplayCommand::QueryQuest { .. }
        | GameplayCommand::AcceptQuest { .. }
        | GameplayCommand::TurnInQuest { .. }
        | GameplayCommand::RequestQuestReward { .. }
        | GameplayCommand::ChooseQuestReward { .. } => PermissionSet::QUEST,
        GameplayCommand::QueryItem { .. } => PermissionSet::empty(),
        GameplayCommand::VendorList { .. }
        | GameplayCommand::VendorBuy { .. }
        | GameplayCommand::VendorSell { .. } => PermissionSet::ECONOMY,
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
    if matches!(
        command,
        GameplayCommand::EquipItem { .. }
            | GameplayCommand::RepairEquipment { .. }
            | GameplayCommand::PetSetReaction { .. }
            | GameplayCommand::PetSetAutocast { .. }
            | GameplayCommand::CastOnItem { .. }
            | GameplayCommand::UseItemOnItem { .. }
            | GameplayCommand::PetAttack { .. }
    ) && !matches!(origin, PlanOrigin::SystemPolicy | PlanOrigin::Operator)
    {
        return false;
    }
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
                | GameplayCommand::SummonPet { .. }
                | GameplayCommand::EquipItem { .. }
                | GameplayCommand::RepairEquipment { .. }
                | GameplayCommand::PetSetReaction { .. }
                | GameplayCommand::PetSetAutocast { .. }
                | GameplayCommand::CastOnItem { .. }
                | GameplayCommand::UseItemOnItem { .. }
        ),
        ActivationStage::Move => matches!(
            command,
            GameplayCommand::Chat { .. }
                | GameplayCommand::UseItem { .. }
                | GameplayCommand::UseItemInstance { .. }
                | GameplayCommand::MaintainBuff { .. }
                | GameplayCommand::SummonPet { .. }
                | GameplayCommand::EquipItem { .. }
                | GameplayCommand::RepairEquipment { .. }
                | GameplayCommand::PetSetReaction { .. }
                | GameplayCommand::PetSetAutocast { .. }
                | GameplayCommand::CastOnItem { .. }
                | GameplayCommand::UseItemOnItem { .. }
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
    fn unknown_spell_is_rejected_before_cast_dispatch() {
        let action = ProposedAction {
            id: ActionId(3),
            task: TaskId(3),
            origin: PlanOrigin::Operator,
            stamp: base_stamp(),
            command: GameplayCommand::Cast {
                spell: 172,
                target: None,
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
            matches!(outcome, ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "unknown_spell")
        );
    }

    #[test]
    fn repair_requires_observed_durability_that_needs_repair() {
        use wow_state::{
            entities::{EntityKind, EntityState},
            inventory::EquipmentCondition,
        };

        let repair_entry = wow_infra::world_knowledge::embedded_azerothcore_catalog()
            .world()
            .vendor_services
            .iter()
            .find(|service| service.can_repair)
            .expect("repair vendor catalog entry")
            .entry_id;
        let mut state = AuthoritativeState::default();
        state.revision = base_stamp().state;
        state.session.in_world = true;
        state.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.inventory.equipment_condition = EquipmentCondition {
            observed: true,
            lowest_durability_percent: Some(100),
            broken_items: 0,
        };
        state.entities.0.insert(
            EntityId(2),
            EntityState {
                id: EntityId(2),
                kind: EntityKind::Unit,
                entry: repair_entry,
                interactable: true,
                position: state.position.player,
                ..Default::default()
            },
        );
        let action = ProposedAction {
            id: ActionId(4),
            task: TaskId(4),
            origin: PlanOrigin::SystemPolicy,
            stamp: base_stamp(),
            command: GameplayCommand::RepairEquipment {
                vendor: EntityId(2),
            },
        };
        let outcome = ActionValidator::validate(
            &Snapshot::from_state(&state),
            ValidationContext {
                current: base_stamp(),
                stage: ActivationStage::Act,
                permissions: PermissionSet::MAINTENANCE,
            },
            action,
        );
        assert!(matches!(
            outcome,
            ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "repair_not_needed"
        ));
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
