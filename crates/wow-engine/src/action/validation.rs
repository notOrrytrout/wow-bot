use super::spatial;
use wow_domain::time::Millis;
use wow_domain::*;
use wow_state::Snapshot;

pub struct ValidationContext {
    pub current: ValidityStamp,
    pub stage: ActivationStage,
    pub permissions: PermissionSet,
    pub bank_keep_item_ids: Vec<u32>,
    pub auto_professions_enabled: bool,
    pub battleground_pvp_authorized: bool,
    pub fresh_group_loot_rolls: Vec<(EntityId, u32)>,
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

        if !context.auto_professions_enabled
            && matches!(
                action.command,
                GameplayCommand::TrainerBuy { trainer, .. }
                    if wow_policy::gathering::professions::is_nearby_profession_trainer(
                        snapshot, trainer
                    )
            )
        {
            return reject(
                "profession_training_disabled",
                "automatic profession training is disabled",
                false,
            );
        }

        if let Err(outcome) = validate_command_with_battleground(
            snapshot,
            &action.command,
            context.battleground_pvp_authorized,
            &context.fresh_group_loot_rolls,
        ) {
            return outcome;
        }
        if let Err(outcome) =
            validate_bank_keep_item_ids(snapshot, &action.command, &context.bank_keep_item_ids)
        {
            return outcome;
        }
        send(action)
    }
}

fn validate_bank_keep_item_ids(
    snapshot: &Snapshot,
    command: &GameplayCommand,
    keep_item_ids: &[u32],
) -> Result<(), ValidationOutcome> {
    let keep = wow_policy::economy::bank::protected_item_id_set(keep_item_ids);
    match command {
        GameplayCommand::BankActivate { banker }
            if wow_policy::economy::bank::profession_material_deposit_candidates(
                snapshot, *banker, &keep,
            )
            .is_empty() =>
        {
            Err(reject(
                "unsafe_bank_item",
                "no safe backpack profession material remains after configured keep IDs",
                true,
            ))
        }
        GameplayCommand::BankDeposit { item, .. } if keep.contains(item) => Err(reject(
            "protected_bank_item",
            "configured keep IDs cannot be deposited",
            true,
        )),
        _ => Ok(()),
    }
}

fn validate_command(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Result<(), ValidationOutcome> {
    validate_command_with_battleground(snapshot, command, false, &[])
}

fn validate_command_with_battleground(
    snapshot: &Snapshot,
    command: &GameplayCommand,
    battleground_pvp_authorized: bool,
    fresh_group_loot_rolls: &[(EntityId, u32)],
) -> Result<(), ValidationOutcome> {
    validate_spatial_command(snapshot, command, battleground_pvp_authorized)?;
    validate_battleground_command(snapshot, command)?;
    validate_group_loot_command(snapshot, command, fresh_group_loot_rolls)?;
    validate_quest_command(snapshot, command)?;
    validate_economy_command(snapshot, command)
}

fn validate_battleground_command(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Result<(), ValidationOutcome> {
    use wow_state::battleground::BattlegroundQueueStatus as Status;

    let queues = &snapshot.state.battleground.queues;
    let current_type = |status: Status, type_id: u32| {
        queues
            .values()
            .any(|queue| queue.status == status && queue.battleground_type_id == Some(type_id))
    };
    let invalid = match command {
        GameplayCommand::BattlegroundJoinRandom => !queues.is_empty(),
        GameplayCommand::BattlegroundPort {
            battleground_type_id,
            enter: true,
        } => !current_type(Status::WaitJoin, *battleground_type_id),
        GameplayCommand::BattlegroundPort {
            battleground_type_id,
            enter: false,
        } => ![Status::WaitQueue, Status::WaitJoin]
            .into_iter()
            .any(|status| current_type(status, *battleground_type_id)),
        GameplayCommand::BattlegroundLeave {
            battleground_type_id,
        } => ![Status::InProgress, Status::WaitLeave]
            .into_iter()
            .any(|status| current_type(status, *battleground_type_id)),
        GameplayCommand::BattlegroundStatus => false,
        _ => return Ok(()),
    };
    if invalid {
        return Err(reject(
            "battleground_state",
            "action does not match current authoritative battleground queue state",
            true,
        ));
    }
    Ok(())
}

fn validate_group_loot_command(
    snapshot: &Snapshot,
    command: &GameplayCommand,
    fresh_rolls: &[(EntityId, u32)],
) -> Result<(), ValidationOutcome> {
    let GameplayCommand::LootRollVote {
        item,
        item_slot,
        choice,
    } = command
    else {
        return Ok(());
    };
    let choice = choice.wire_value();
    let method_allows_roll = matches!(
        snapshot.state.group.loot_method,
        Some(
            wow_state::group::GroupLootMethod::GroupLoot
                | wow_state::group::GroupLootMethod::NeedBeforeGreed
        )
    );
    let request_allows_vote = snapshot.state.group.loot_rolls.iter().any(|request| {
        request.item == *item && request.item_slot == *item_slot && request.allows(choice)
    });
    if !method_allows_roll || !request_allows_vote || !fresh_rolls.contains(&(*item, *item_slot)) {
        return Err(reject(
            "group_loot_roll_state",
            "vote does not match a fresh, active server roll request",
            true,
        ));
    }
    Ok(())
}

fn has_unique_nearby_trusted_mailbox(snapshot: &Snapshot, mailbox: EntityId) -> bool {
    let Some(player) = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)
    else {
        return false;
    };
    let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
    let mut matches = snapshot.state.entities.0.values().filter(|entity| {
        entity.kind == wow_state::entities::EntityKind::GameObject
            && entity.interactable
            && catalog
                .gameobject_name(entity.entry)
                .is_some_and(|name| name.to_ascii_lowercase().contains("mailbox"))
            && entity.position.is_some_and(|position| {
                position.map == player.map && position.point.distance(player.point) <= 5.0
            })
    });
    matches.next().is_some_and(|entity| entity.id == mailbox) && matches.next().is_none()
}

fn validate_spatial_command(
    snapshot: &Snapshot,
    command: &GameplayCommand,
    battleground_pvp_authorized: bool,
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
            if !attack_authorized(snapshot, *entity, battleground_pvp_authorized) =>
        {
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
            if !attack_authorized(snapshot, *target, battleground_pvp_authorized) {
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
            if let Some(target) = target {
                validate_hostile_player_spell_target(
                    snapshot,
                    *target,
                    battleground_pvp_authorized,
                )?;
            }
        }
        GameplayCommand::VehicleCast {
            target: Some(target),
            ..
        } => validate_hostile_player_spell_target(snapshot, *target, battleground_pvp_authorized)?,
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
        GameplayCommand::SetAmmo { item } => {
            let inventory = &snapshot.state.inventory;
            let present_in_bags = inventory.instances_authoritative
                && inventory.instances.values().any(|instance| {
                    instance.item == *item && instance.backpack_slot >= 23 && instance.count > 0
                });
            if !present_in_bags
                || !wow_policy::maintenance::ranged_ammo_item_is_compatible(snapshot, *item)
            {
                return Err(reject(
                    "invalid_ammo_selection",
                    "compatible ammo is not present in authoritative inventory",
                    true,
                ));
            }
        }
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
        GameplayCommand::TrainerList { trainer } => {
            if !is_nearby_class_trainer(snapshot, *trainer)
                && !wow_policy::gathering::professions::is_nearby_profession_trainer(
                    snapshot, *trainer,
                )
            {
                return Err(reject(
                    "trainer_not_available",
                    "class trainer is not currently observed and in interaction range",
                    true,
                ));
            }
        }
        GameplayCommand::TrainerBuy { trainer, spell } => {
            let offer = snapshot
                .state
                .trainer
                .offers
                .iter()
                .find(|offer| offer.spell == *spell);
            let player_level = snapshot
                .state
                .session
                .character_guid
                .map(EntityId)
                .and_then(|player| snapshot.state.entities.0.get(&player))
                .and_then(|player| player.level);
            let required_skill_is_known = offer.is_some_and(|offer| {
                offer.required_skill_line == 0
                    || (snapshot.state.professions.known
                        && u32::from(snapshot.state.professions.skill(offer.required_skill_line))
                            >= offer.required_skill_rank)
            });
            let class_trainer = snapshot.state.trainer.trainer_type == Some(0)
                && is_nearby_class_trainer(snapshot, *trainer);
            let profession_trainer = snapshot.state.trainer.trainer_type == Some(2)
                && wow_policy::gathering::professions::is_nearby_profession_trainer(
                    snapshot, *trainer,
                )
                && wow_policy::gathering::professions::training_state_safe(snapshot);
            let valid = snapshot.state.trainer.trainer == Some(*trainer)
                && (class_trainer || profession_trainer)
                && offer.is_some_and(|offer| {
                    offer.usable == 0
                        && player_level.is_some_and(|level| offer.required_level as u32 <= level)
                        && !snapshot.state.capabilities.spells.contains(&offer.spell)
                        && u64::from(offer.cost_copper)
                            .saturating_add(wow_domain::MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER)
                            <= snapshot.state.inventory.money
                })
                && required_skill_is_known
                && (class_trainer
                    || offer.is_some_and(|offer| {
                        wow_policy::gathering::professions::offer_skill(
                            snapshot,
                            *trainer,
                            offer.spell,
                        )
                        .is_some()
                    }));
            if !valid {
                return Err(reject(
                    "invalid_trainer_offer",
                    "training purchase is not in the current affordable and eligible trainer list",
                    true,
                ));
            }
        }
        GameplayCommand::VendorBuy {
            vendor,
            item,
            slot,
            count,
        } => {
            let offer = snapshot
                .state
                .inventory
                .vendor_inventory
                .as_ref()
                .filter(|inventory| inventory.vendor == *vendor)
                .and_then(|inventory| inventory.offers.iter().find(|offer| offer.slot == *slot));
            let valid = offer.is_some_and(|offer| {
                offer.item == *item
                    && (1..=wow_domain::MAX_MAINTENANCE_VENDOR_BUY_LOTS).contains(count)
                    && offer.buy_count > 0
                    && offer.extended_cost == 0
                    && offer.stock.is_none_or(|stock| {
                        offer
                            .buy_count
                            .checked_mul(*count)
                            .is_some_and(|required_stock| stock >= required_stock)
                    })
                    && u64::from(offer.price_copper)
                        .saturating_mul(u64::from(*count))
                        .saturating_add(MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER)
                        <= snapshot.state.inventory.money
            });
            let metadata = snapshot.state.inventory.item_metadata.get(item);
            let poison = metadata.is_some_and(|metadata| {
                metadata.item_class == 0
                    && metadata.subclass == 6
                    && {
                        let name = metadata.name.to_ascii_lowercase();
                        name.contains("instant poison") || name.contains("deadly poison")
                    }
                    && metadata.required_level
                        <= snapshot
                            .state
                            .session
                            .character_guid
                            .map(EntityId)
                            .and_then(|player| snapshot.state.entities.0.get(&player))
                            .and_then(|player| player.level)
                            .unwrap_or(0)
                    && (metadata.allowable_class == 0
                        || metadata.allowable_class == u32::MAX
                        || metadata.allowable_class & (1 << (4 - 1)) != 0)
            });
            let recovery_supply = metadata.is_some_and(|metadata| {
                snapshot
                    .state
                    .capabilities
                    .class_id
                    .zip(
                        snapshot
                            .state
                            .session
                            .character_guid
                            .map(EntityId)
                            .and_then(|player| snapshot.state.entities.0.get(&player))
                            .and_then(|player| player.level),
                    )
                    .is_some_and(|(class, level)| {
                        wow_policy::maintenance::recovery_supply_item_is_eligible(
                            metadata, class, level,
                        )
                    })
            });
            let ranged_ammo =
                wow_policy::maintenance::ranged_ammo_item_is_compatible(snapshot, *item);
            let vendor_gear = snapshot
                .state
                .session
                .character_guid
                .map(EntityId)
                .is_some_and(|player| {
                    wow_policy::maintenance::vendor_gear_purchase_allowed(snapshot, player)
                })
                && u64::from(offer.map_or(0, |offer| offer.price_copper))
                    <= wow_policy::maintenance::vendor_gear_purchase_budget(
                        snapshot.state.inventory.money,
                    )
                && wow_policy::maintenance::vendor_gear_upgrade_destination(snapshot, *item)
                    .is_some();
            let trusted_seller = snapshot.state.entities.0.get(vendor).is_some_and(|entity| {
                let nearby = snapshot
                    .state
                    .position
                    .player
                    .zip(entity.position)
                    .is_some_and(|(player, seller)| {
                        player.map == seller.map && player.point.distance(seller.point) <= 5.0
                    });
                entity.kind == wow_state::entities::EntityKind::Unit
                    && entity.interactable
                    && nearby
                    && wow_infra::world_knowledge::embedded_azerothcore_catalog()
                        .world()
                        .vendor_services
                        .iter()
                        .any(|service| service.entry_id == entity.entry && service.can_sell)
            });
            if !valid
                || (!poison && !recovery_supply && !ranged_ammo && !vendor_gear)
                || (poison && snapshot.state.capabilities.class_id != Some(4))
                || !trusted_seller
            {
                return Err(reject(
                    "invalid_vendor_offer",
                    "purchase does not match an affordable, observed vendor offer",
                    true,
                ));
            }
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
        GameplayCommand::BankActivate { banker } => {
            if !wow_policy::economy::bank::bag_pressure_requires_bank(snapshot)
                || wow_policy::economy::bank::profession_material_deposit_candidates(
                    snapshot,
                    *banker,
                    &Default::default(),
                )
                .is_empty()
                || !wow_policy::economy::bank::trusted_nearby_bankers(snapshot).contains(banker)
            {
                return Err(reject(
                    "bank_not_available",
                    "severe bag pressure, a safe deposit item, and a nearby observed banker are required",
                    true,
                ));
            }
        }
        GameplayCommand::BankDeposit {
            banker,
            item,
            item_guid,
            backpack_slot,
        } => {
            if !snapshot.state.inventory.bank.authoritative
                || snapshot.state.inventory.bank.banker != Some(*banker)
                || !wow_policy::economy::bank::bag_pressure_requires_bank(snapshot)
                || !wow_policy::economy::bank::trusted_nearby_bankers(snapshot).contains(banker)
            {
                return Err(reject(
                    "stale_bank",
                    "the same banker must remain open and in interaction range under severe bag pressure",
                    true,
                ));
            }
            let candidate = wow_policy::economy::bank::profession_material_deposit_candidates(
                snapshot,
                *banker,
                &Default::default(),
            )
            .into_iter()
            .any(|candidate| {
                candidate.item == *item
                    && candidate.item_guid == *item_guid
                    && candidate.backpack_slot == *backpack_slot
            });
            if !candidate {
                return Err(reject(
                    "unsafe_bank_item",
                    "item is not a current unprotected backpack profession material",
                    true,
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
        GameplayCommand::MailboxList { mailbox } => {
            if !has_unique_nearby_trusted_mailbox(snapshot, *mailbox) {
                return Err(reject(
                    "mailbox_not_available",
                    "mailbox is not a trusted observed object in interaction range",
                    true,
                ));
            }
        }
        GameplayCommand::MailTake {
            mailbox_generation,
            mailbox,
            mail_id,
            target,
        } => {
            if snapshot.state.inventory.mailbox.generation != *mailbox_generation
                || !snapshot.state.inventory.mailbox.authoritative
            {
                return Err(reject("stale_mail", "mailbox contents changed", true));
            }
            if !has_unique_nearby_trusted_mailbox(snapshot, *mailbox) {
                return Err(reject(
                    "mailbox_not_available",
                    "mailbox is not a trusted observed object in interaction range",
                    true,
                ));
            }
            let Some(mail) = snapshot.state.inventory.mailbox.mails.get(mail_id) else {
                return Err(reject("stale_mail", "mail is no longer present", true));
            };
            if mail.cod_copper != Some(0) {
                return Err(reject(
                    "mail_cod_unknown_or_present",
                    "mail COD status is not authoritatively zero",
                    false,
                ));
            }
            match target {
                MailTakeTarget::Money if mail.money == 0 => {
                    return Err(reject(
                        "mail_money_missing",
                        "mail has no attached money",
                        true,
                    ));
                }
                MailTakeTarget::Attachment { low_guid } => {
                    if snapshot.state.inventory.free_slots == 0 {
                        return Err(reject("mail_no_bag_space", "no free backpack slot", true));
                    }
                    if !mail.attachments.contains_key(low_guid) {
                        return Err(reject(
                            "mail_attachment_missing",
                            "mail attachment is no longer present",
                            true,
                        ));
                    }
                }
                _ => {}
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

fn is_nearby_class_trainer(snapshot: &Snapshot, trainer: EntityId) -> bool {
    let Some(entity) = snapshot.state.entities.0.get(&trainer) else {
        return false;
    };
    let Some(position) = entity.position else {
        return false;
    };
    let Some(player_position) = snapshot.state.position.player else {
        return false;
    };
    entity.kind == wow_state::entities::EntityKind::Unit
        && entity.interactable
        && entity
            .npc_flags
            .is_some_and(|flags| flags & 0x20 != 0 || (flags & 0x10 != 0 && flags & 0x40 == 0))
        && position.map == player_position.map
        && position.point.distance(player_position.point) <= 5.0
}

fn attack_authorized(
    snapshot: &Snapshot,
    target: EntityId,
    battleground_pvp_authorized: bool,
) -> bool {
    snapshot
        .state
        .entities
        .0
        .get(&target)
        .is_some_and(|entity| {
            let engaged =
                wow_policy::combat::engagement::is_attacking_player_or_group(snapshot, target);
            if entity.kind == wow_state::entities::EntityKind::Player {
                return engaged
                    || (battleground_pvp_authorized
                        && entity.hostile
                        && snapshot.state.position.player.is_some_and(|position| {
                            wow_policy::battleground::is_wotlk_battleground_map(position.map)
                        }));
            }
            entity.hostile || active_quest_authorizes_attack(snapshot, entity.entry) || engaged
        })
}

fn validate_hostile_player_spell_target(
    snapshot: &Snapshot,
    target: EntityId,
    battleground_pvp_authorized: bool,
) -> Result<(), ValidationOutcome> {
    if snapshot
        .state
        .entities
        .0
        .get(&target)
        .is_some_and(|entity| {
            entity.kind == wow_state::entities::EntityKind::Player && entity.hostile
        })
        && !attack_authorized(snapshot, target, battleground_pvp_authorized)
    {
        return Err(reject(
            "combat_authority",
            "hostile player spell target is not authorized for battleground combat",
            false,
        ));
    }
    Ok(())
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
        | GameplayCommand::StopMovement
        | GameplayCommand::CancelMount
        | GameplayCommand::CancelAura { .. } => PermissionSet::MOVE,
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
        GameplayCommand::EquipItem { .. } | GameplayCommand::SetAmmo { .. } => {
            PermissionSet::MAINTENANCE
        }
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
        | GameplayCommand::TrainerList { .. }
        | GameplayCommand::TrainerBuy { .. } => PermissionSet::MAINTENANCE,
        GameplayCommand::VendorSell { .. } => PermissionSet::ECONOMY,
        GameplayCommand::TradeAccept { .. }
        | GameplayCommand::AuctionBuy { .. }
        | GameplayCommand::MailTake { .. } => {
            PermissionSet::ECONOMY | PermissionSet::ASSET_TRANSFER
        }
        GameplayCommand::MailboxList { .. } => PermissionSet::ECONOMY,
        GameplayCommand::BankActivate { .. } => PermissionSet::MAINTENANCE,
        GameplayCommand::BankDeposit { .. } => PermissionSet::MAINTENANCE,
        GameplayCommand::BattlegroundJoinRandom
        | GameplayCommand::BattlegroundStatus
        | GameplayCommand::BattlegroundPort { .. }
        | GameplayCommand::BattlegroundLeave { .. } => PermissionSet::GROUP,
        GameplayCommand::LootRollVote { .. } => PermissionSet::GROUP,
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
        GameplayCommand::CancelMount
            | GameplayCommand::CancelAura { .. }
            | GameplayCommand::EquipItem { .. }
            | GameplayCommand::RepairEquipment { .. }
            | GameplayCommand::SetAmmo { .. }
            | GameplayCommand::PetSetReaction { .. }
            | GameplayCommand::PetSetAutocast { .. }
            | GameplayCommand::CastOnItem { .. }
            | GameplayCommand::UseItemOnItem { .. }
            | GameplayCommand::VendorList { .. }
            | GameplayCommand::VendorBuy { .. }
            | GameplayCommand::TrainerList { .. }
            | GameplayCommand::TrainerBuy { .. }
            | GameplayCommand::MailboxList { .. }
            | GameplayCommand::MailTake { .. }
            | GameplayCommand::BankActivate { .. }
            | GameplayCommand::BankDeposit { .. }
            | GameplayCommand::BattlegroundJoinRandom
            | GameplayCommand::BattlegroundStatus
            | GameplayCommand::BattlegroundPort { .. }
            | GameplayCommand::BattlegroundLeave { .. }
            | GameplayCommand::LootRollVote { .. }
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
                | GameplayCommand::BankActivate { .. }
                | GameplayCommand::BankDeposit { .. }
                | GameplayCommand::SetAmmo { .. }
                | GameplayCommand::PetSetReaction { .. }
                | GameplayCommand::PetSetAutocast { .. }
                | GameplayCommand::CastOnItem { .. }
                | GameplayCommand::UseItemOnItem { .. }
                | GameplayCommand::VendorList { .. }
                | GameplayCommand::VendorBuy { .. }
                | GameplayCommand::TrainerList { .. }
                | GameplayCommand::TrainerBuy { .. }
                | GameplayCommand::MailboxList { .. }
                | GameplayCommand::QueryItem { .. }
                | GameplayCommand::CancelMount
                | GameplayCommand::CancelAura { .. }
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
                | GameplayCommand::BankActivate { .. }
                | GameplayCommand::BankDeposit { .. }
                | GameplayCommand::SetAmmo { .. }
                | GameplayCommand::PetSetReaction { .. }
                | GameplayCommand::PetSetAutocast { .. }
                | GameplayCommand::CastOnItem { .. }
                | GameplayCommand::UseItemOnItem { .. }
                | GameplayCommand::VendorList { .. }
                | GameplayCommand::VendorBuy { .. }
                | GameplayCommand::TrainerList { .. }
                | GameplayCommand::TrainerBuy { .. }
                | GameplayCommand::MailboxList { .. }
                | GameplayCommand::QueryItem { .. }
                | GameplayCommand::MoveTo(_)
                | GameplayCommand::FaceDirection { .. }
                | GameplayCommand::StopMovement
                | GameplayCommand::CancelMount
                | GameplayCommand::CancelAura { .. }
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
    use wow_state::{
        AuthoritativeState,
        entities::{EntityKind, EntityState},
    };

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

    fn battleground_snapshot(
        status: wow_state::battleground::BattlegroundQueueStatus,
        battleground_type_id: Option<u32>,
    ) -> Snapshot {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.battleground.queues.insert(
            0,
            wow_state::battleground::BattlegroundQueueState {
                queue_slot: 0,
                battleground_type_id,
                status,
                map_id: None,
                invite_timeout_ms: None,
                elapsed_time_ms: None,
                auto_leave_time_ms: None,
                team_alliance: None,
            },
        );
        Snapshot::from_state(&state)
    }

    #[test]
    fn battleground_commands_require_matching_observed_queue_state() {
        use wow_state::battleground::BattlegroundQueueStatus as Status;

        assert!(
            validate_battleground_command(&snapshot(), &GameplayCommand::BattlegroundStatus)
                .is_ok()
        );
        assert!(
            validate_battleground_command(&snapshot(), &GameplayCommand::BattlegroundJoinRandom)
                .is_ok()
        );
        assert!(
            validate_battleground_command(
                &battleground_snapshot(Status::WaitQueue, Some(32)),
                &GameplayCommand::BattlegroundJoinRandom
            )
            .is_err()
        );
        assert!(
            validate_battleground_command(
                &battleground_snapshot(Status::WaitJoin, Some(32)),
                &GameplayCommand::BattlegroundPort {
                    battleground_type_id: 31,
                    enter: true,
                }
            )
            .is_err()
        );
        assert!(
            validate_battleground_command(
                &battleground_snapshot(Status::WaitJoin, Some(32)),
                &GameplayCommand::BattlegroundPort {
                    battleground_type_id: 32,
                    enter: true,
                }
            )
            .is_ok()
        );
        assert!(
            validate_battleground_command(
                &battleground_snapshot(Status::InProgress, Some(32)),
                &GameplayCommand::BattlegroundPort {
                    battleground_type_id: 32,
                    enter: false,
                }
            )
            .is_err()
        );
        assert!(
            validate_battleground_command(
                &battleground_snapshot(Status::WaitQueue, Some(32)),
                &GameplayCommand::BattlegroundPort {
                    battleground_type_id: 32,
                    enter: false,
                }
            )
            .is_ok()
        );
        assert!(
            validate_battleground_command(
                &battleground_snapshot(Status::InProgress, Some(32)),
                &GameplayCommand::BattlegroundLeave {
                    battleground_type_id: 32,
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn proactive_player_attacks_require_authorized_battleground_context() {
        let mut state = AuthoritativeState::default();
        state.revision = StateRevision(7);
        state.session.in_world = true;
        state.position.player = Some(WorldPosition {
            map: 489,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.entities.0.insert(
            EntityId(44),
            EntityState {
                id: EntityId(44),
                kind: EntityKind::Player,
                hostile: true,
                position: Some(WorldPosition {
                    map: 489,
                    point: Vec3::new(2.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..EntityState::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        let action = ProposedAction {
            id: ActionId(1),
            task: TaskId(1),
            origin: PlanOrigin::SystemPolicy,
            stamp: base_stamp(),
            command: GameplayCommand::Attack(EntityId(44)),
        };
        let context = |battleground_pvp_authorized| ValidationContext {
            current: base_stamp(),
            stage: ActivationStage::Act,
            bank_keep_item_ids: Vec::new(),
            auto_professions_enabled: true,
            battleground_pvp_authorized,
            fresh_group_loot_rolls: Vec::new(),
            permissions: PermissionSet::COMBAT,
        };
        assert!(matches!(
            ActionValidator::validate(&snapshot, context(false), action.clone()),
            ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "combat_authority"
        ));
        assert!(matches!(
            ActionValidator::validate(&snapshot, context(true), action.clone()),
            ValidationOutcome::Sendable(_)
        ));

        let mut arena = state;
        arena.position.player.as_mut().unwrap().map = 559;
        arena
            .entities
            .0
            .get_mut(&EntityId(44))
            .unwrap()
            .position
            .as_mut()
            .unwrap()
            .map = 559;
        assert!(matches!(
            ActionValidator::validate(&Snapshot::from_state(&arena), context(true), action),
            ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "combat_authority"
        ));
    }

    #[test]
    fn group_loot_vote_requires_a_fresh_server_request_and_allowed_choice() {
        let mut state = AuthoritativeState::default();
        state.revision = StateRevision(7);
        state.session.in_world = true;
        state.group.loot_method = Some(wow_state::group::GroupLootMethod::NeedBeforeGreed);
        state
            .group
            .loot_rolls
            .push(wow_state::group::GroupLootRollRequest {
                item: EntityId(77),
                map_id: 571,
                item_slot: 2,
                item_id: 123,
                item_count: 1,
                countdown_ms: 30_000,
                vote_mask: 0b1001,
            });
        let snapshot = Snapshot::from_state(&state);
        let action = ProposedAction {
            id: ActionId(1),
            task: TaskId(1),
            origin: PlanOrigin::SystemPolicy,
            stamp: base_stamp(),
            command: GameplayCommand::LootRollVote {
                item: EntityId(77),
                item_slot: 2,
                choice: LootRollChoice::Pass,
            },
        };
        let context = |fresh_group_loot_rolls| ValidationContext {
            current: base_stamp(),
            stage: ActivationStage::Act,
            bank_keep_item_ids: Vec::new(),
            auto_professions_enabled: true,
            battleground_pvp_authorized: false,
            fresh_group_loot_rolls,
            permissions: PermissionSet::GROUP,
        };
        assert!(matches!(
            ActionValidator::validate(&snapshot, context(vec![(EntityId(77), 2)]), action.clone()),
            ValidationOutcome::Sendable(_)
        ));
        assert!(matches!(
            ActionValidator::validate(&snapshot, context(Vec::new()), action.clone()),
            ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "group_loot_roll_state"
        ));
        let invalid_choice = ProposedAction {
            command: GameplayCommand::LootRollVote {
                item: EntityId(77),
                item_slot: 2,
                choice: LootRollChoice::Need,
            },
            ..action
        };
        assert!(matches!(
            ActionValidator::validate(&snapshot, context(vec![(EntityId(77), 2)]), invalid_choice),
            ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "group_loot_roll_state"
        ));
    }

    #[test]
    fn battleground_commands_require_group_permission_and_system_policy_origin() {
        assert_eq!(
            required_permission(&GameplayCommand::BattlegroundJoinRandom),
            PermissionSet::GROUP
        );
        assert!(!origin_authorized(
            PlanOrigin::Llm,
            &GameplayCommand::BattlegroundJoinRandom
        ));
        assert!(!origin_authorized(
            PlanOrigin::GroupPolicy,
            &GameplayCommand::BattlegroundStatus
        ));
        assert!(origin_authorized(
            PlanOrigin::SystemPolicy,
            &GameplayCommand::BattlegroundStatus
        ));
    }

    fn mailbox_snapshot(cod: Option<u64>, free_slots: u16) -> Snapshot {
        use wow_state::inventory::{MailAttachment, MailEntry};

        let mut state = AuthoritativeState::default();
        state.revision = StateRevision(7);
        state.session.in_world = true;
        state.position.player = Some(WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.inventory.free_slots = free_slots;
        state.inventory.mailbox.generation = 8;
        state.inventory.mailbox.authoritative = true;
        state.inventory.mailbox.mails.insert(
            77,
            MailEntry {
                mail_id: 77,
                money: 12_345,
                cod_copper: cod,
                attachments: [(
                    9001,
                    MailAttachment {
                        item_id: 4306,
                        count: 4,
                    },
                )]
                .into_iter()
                .collect(),
            },
        );
        state.entities.0.insert(
            EntityId(99),
            EntityState {
                id: EntityId(99),
                entry: 32349,
                kind: EntityKind::GameObject,
                position: state.position.player,
                interactable: true,
                ..EntityState::default()
            },
        );
        Snapshot::from_state(&state)
    }

    fn bank_snapshot(open: bool, free_slots: u16) -> (Snapshot, EntityId, EntityId) {
        use wow_state::inventory::{InventoryItemInstance, ItemTemplateMetadata};

        let banker = EntityId(44);
        let item_guid = EntityId(90);
        let position = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut state = AuthoritativeState::default();
        state.revision = StateRevision(7);
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.position.player = Some(position);
        state.inventory.instances_authoritative = true;
        state.inventory.free_slots = free_slots;
        state.inventory.bank = wow_state::inventory::BankState {
            authoritative: open,
            banker: open.then_some(banker),
        };
        state.inventory.instances.insert(
            item_guid,
            InventoryItemInstance {
                item: 2589,
                guid: item_guid,
                backpack_slot: 23,
                count: 4,
            },
        );
        state.inventory.item_metadata.insert(
            2589,
            ItemTemplateMetadata {
                item_class: 7,
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                health: Some((100, 100)),
                position: Some(position),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            banker,
            EntityState {
                id: banker,
                kind: EntityKind::Unit,
                npc_flags: Some(0x8),
                interactable: true,
                position: Some(position),
                ..Default::default()
            },
        );
        (Snapshot::from_state(&state), banker, item_guid)
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
                bank_keep_item_ids: Vec::new(),
                auto_professions_enabled: true,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
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
                bank_keep_item_ids: Vec::new(),
                auto_professions_enabled: true,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
                permissions: PermissionSet::ALL,
            },
            action,
        );
        assert!(
            matches!(outcome, ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "unknown_spell")
        );
    }

    #[test]
    fn mail_collection_requires_known_non_cod_and_current_contents() {
        let mailbox = EntityId(99);
        let money_action = GameplayCommand::MailTake {
            mailbox_generation: 8,
            mailbox,
            mail_id: 77,
            target: MailTakeTarget::Money,
        };
        let safe = mailbox_snapshot(Some(0), 1);
        assert!(validate_command(&safe, &money_action).is_ok());

        for cod in [None, Some(1)] {
            assert!(matches!(
                validate_command(&mailbox_snapshot(cod, 1), &money_action),
                Err(ValidationOutcome::Rejected(ActionFailure {
                    code,
                    ..
                })) if code == "mail_cod_unknown_or_present"
            ));
        }

        let attachment_action = GameplayCommand::MailTake {
            mailbox_generation: 8,
            mailbox,
            mail_id: 77,
            target: MailTakeTarget::Attachment { low_guid: 9001 },
        };
        assert!(validate_command(&safe, &attachment_action).is_ok());
        assert!(matches!(
            validate_command(&mailbox_snapshot(Some(0), 0), &attachment_action),
            Err(ValidationOutcome::Rejected(ActionFailure { code, .. }))
                if code == "mail_no_bag_space"
        ));

        let stale = GameplayCommand::MailTake {
            mailbox_generation: 7,
            mailbox,
            mail_id: 77,
            target: MailTakeTarget::Money,
        };
        assert!(matches!(
            validate_command(&safe, &stale),
            Err(ValidationOutcome::Rejected(ActionFailure { code, .. }))
                if code == "stale_mail"
        ));
    }

    #[test]
    fn mail_collection_requires_the_unique_trusted_mailbox_in_range() {
        let mailbox = EntityId(99);
        let action = GameplayCommand::MailTake {
            mailbox_generation: 8,
            mailbox,
            mail_id: 77,
            target: MailTakeTarget::Money,
        };
        let mut far = mailbox_snapshot(Some(0), 1);
        far.state
            .entities
            .0
            .get_mut(&mailbox)
            .unwrap()
            .position
            .as_mut()
            .unwrap()
            .point
            .x = 6.0;
        assert!(matches!(
            validate_command(&far, &action),
            Err(ValidationOutcome::Rejected(ActionFailure { code, .. }))
                if code == "mailbox_not_available"
        ));
        let mut untrusted = mailbox_snapshot(Some(0), 1);
        untrusted.state.entities.0.get_mut(&mailbox).unwrap().entry = 123;
        assert!(matches!(
            validate_command(&untrusted, &action),
            Err(ValidationOutcome::Rejected(ActionFailure { code, .. }))
                if code == "mailbox_not_available"
        ));
        let mut ambiguous = mailbox_snapshot(Some(0), 1);
        ambiguous.state.entities.0.insert(
            EntityId(100),
            EntityState {
                id: EntityId(100),
                entry: 32349,
                kind: EntityKind::GameObject,
                position: ambiguous.state.position.player,
                interactable: true,
                ..EntityState::default()
            },
        );
        assert!(matches!(
            validate_command(&ambiguous, &action),
            Err(ValidationOutcome::Rejected(ActionFailure { code, .. }))
                if code == "mailbox_not_available"
        ));
    }

    #[test]
    fn bank_actions_require_bag_pressure_live_banker_and_matching_open_state() {
        let (closed, banker, item_guid) = bank_snapshot(false, 2);
        assert!(validate_command(&closed, &GameplayCommand::BankActivate { banker }).is_ok());
        let open_action = GameplayCommand::BankDeposit {
            banker,
            item: 2589,
            item_guid,
            backpack_slot: 23,
        };
        assert!(validate_command(&closed, &open_action).is_err());

        let (open, _, _) = bank_snapshot(true, 2);
        assert!(validate_command(&open, &open_action).is_ok());
        assert!(matches!(
            validate_bank_keep_item_ids(&open, &open_action, &[2589]),
            Err(ValidationOutcome::Rejected(ActionFailure { code, .. }))
                if code == "protected_bank_item"
        ));
        assert!(matches!(
            validate_bank_keep_item_ids(&closed, &GameplayCommand::BankActivate { banker }, &[2589]),
            Err(ValidationOutcome::Rejected(ActionFailure { code, .. }))
                if code == "unsafe_bank_item"
        ));
        let (wrong_banker, _, _) = bank_snapshot(true, 2);
        let stale = GameplayCommand::BankDeposit {
            banker: EntityId(45),
            item: 2589,
            item_guid,
            backpack_slot: 23,
        };
        assert!(validate_command(&wrong_banker, &stale).is_err());
        let (no_pressure, _, _) = bank_snapshot(true, 3);
        assert!(validate_command(&no_pressure, &open_action).is_err());
        let mut combat_state = open.state.clone();
        combat_state
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .unit_flags = Some(0x0008_0000);
        assert!(validate_command(&Snapshot::from_state(&combat_state), &open_action).is_err());
        combat_state
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .unit_flags = Some(0);
        combat_state.position.moving = true;
        assert!(validate_command(&Snapshot::from_state(&combat_state), &open_action).is_err());
        let mut far_state = open.state.clone();
        far_state.entities.0.get_mut(&banker).unwrap().position = Some(WorldPosition {
            point: Vec3::new(6.0, 0.0, 0.0),
            ..far_state.position.player.unwrap()
        });
        assert!(validate_command(&Snapshot::from_state(&far_state), &open_action).is_err());
    }

    #[test]
    fn configured_bank_keep_ids_are_rechecked_by_final_action_validation() {
        let (open, banker, item_guid) = bank_snapshot(true, 2);
        let action = ProposedAction {
            id: ActionId(11),
            task: TaskId(12),
            origin: PlanOrigin::SystemPolicy,
            stamp: base_stamp(),
            command: GameplayCommand::BankDeposit {
                banker,
                item: 2589,
                item_guid,
                backpack_slot: 23,
            },
        };
        let result = ActionValidator::validate(
            &open,
            ValidationContext {
                current: base_stamp(),
                stage: ActivationStage::Maintain,
                permissions: PermissionSet::MAINTENANCE,
                bank_keep_item_ids: vec![2589],
                auto_professions_enabled: true,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
            },
            action,
        );
        assert!(matches!(
            result,
            ValidationOutcome::Rejected(ActionFailure { code, .. })
                if code == "protected_bank_item"
        ));
    }

    #[test]
    fn mail_take_requires_the_asset_transfer_permission() {
        let snapshot = mailbox_snapshot(Some(0), 1);
        let action = ProposedAction {
            id: ActionId(30),
            task: TaskId(30),
            origin: PlanOrigin::SystemPolicy,
            stamp: base_stamp(),
            command: GameplayCommand::MailTake {
                mailbox_generation: 8,
                mailbox: EntityId(99),
                mail_id: 77,
                target: MailTakeTarget::Money,
            },
        };
        let restricted = ActionValidator::validate(
            &snapshot,
            ValidationContext {
                current: base_stamp(),
                stage: ActivationStage::Act,
                bank_keep_item_ids: Vec::new(),
                auto_professions_enabled: true,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
                permissions: PermissionSet::ECONOMY,
            },
            action.clone(),
        );
        assert!(matches!(
            restricted,
            ValidationOutcome::Rejected(ActionFailure { code, .. })
                if code == "permission_denied"
        ));
        let allowed = ActionValidator::validate(
            &snapshot,
            ValidationContext {
                current: base_stamp(),
                stage: ActivationStage::Act,
                bank_keep_item_ids: Vec::new(),
                auto_professions_enabled: true,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
                permissions: PermissionSet::ECONOMY | PermissionSet::ASSET_TRANSFER,
            },
            action,
        );
        assert!(matches!(allowed, ValidationOutcome::Sendable(_)));
    }

    #[test]
    fn trainer_purchase_requires_a_current_nearby_affordable_class_offer() {
        let trainer = EntityId(22);
        let player_position = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut state = AuthoritativeState::default();
        state.revision = base_stamp().state;
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.position.player = Some(player_position);
        state.inventory.money = 2_000;
        state.entities.0.insert(
            EntityId(7),
            EntityState {
                id: EntityId(7),
                level: Some(20),
                health: Some((100, 100)),
                position: Some(player_position),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            trainer,
            EntityState {
                id: trainer,
                kind: EntityKind::Unit,
                npc_flags: Some(0x30),
                interactable: true,
                position: Some(player_position),
                ..Default::default()
            },
        );
        state.trainer = wow_state::trainer::TrainerState {
            trainer: Some(trainer),
            trainer_type: Some(0),
            offers: vec![wow_state::trainer::TrainerSpellOffer {
                spell: 1234,
                usable: 0,
                cost_copper: 1_000,
                required_level: 20,
                required_skill_line: 0,
                required_skill_rank: 0,
            }],
        };
        let command = GameplayCommand::TrainerBuy {
            trainer,
            spell: 1234,
        };
        let snapshot = Snapshot::from_state(&state);
        assert!(validate_command(&snapshot, &command).is_ok());

        state.inventory.money = 1_999;
        assert!(validate_command(&Snapshot::from_state(&state), &command).is_err());
        state.inventory.money = 2_000;
        state.capabilities.spells.insert(1234);
        assert!(validate_command(&Snapshot::from_state(&state), &command).is_err());
        state.capabilities.spells.clear();
        state.entities.0.get_mut(&trainer).unwrap().position = Some(WorldPosition {
            point: Vec3::new(6.0, 0.0, 0.0),
            ..player_position
        });
        assert!(validate_command(&Snapshot::from_state(&state), &command).is_err());
    }

    #[test]
    fn profession_purchase_requires_catalog_mapped_spell_and_profession_flags() {
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let (service, mapping) = catalog
            .world()
            .trainer_services
            .iter()
            .find_map(|service| {
                service
                    .spell_skills
                    .iter()
                    .find(|mapping| mapping.skill == 185)
                    .map(|mapping| (service, mapping))
            })
            .expect("cooking trainer spell mapping");
        let trainer = EntityId(22);
        let player_position = WorldPosition {
            map: 0,
            point: Vec3::default(),
            orientation: 0.0,
        };
        let mut state = AuthoritativeState::default();
        state.revision = base_stamp().state;
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.position.player = Some(player_position);
        state.professions.known = true;
        state.inventory.money = 2_000;
        state.entities.0.insert(
            EntityId(7),
            EntityState {
                id: EntityId(7),
                level: Some(20),
                health: Some((100, 100)),
                position: Some(player_position),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            trainer,
            EntityState {
                id: trainer,
                entry: service.entry_id,
                kind: EntityKind::Unit,
                npc_flags: Some(0x50),
                interactable: true,
                position: Some(player_position),
                ..Default::default()
            },
        );
        state.trainer = wow_state::trainer::TrainerState {
            trainer: Some(trainer),
            trainer_type: Some(2),
            offers: vec![wow_state::trainer::TrainerSpellOffer {
                spell: mapping.spell,
                usable: 0,
                cost_copper: 500,
                required_level: 1,
                ..Default::default()
            }],
        };
        let command = GameplayCommand::TrainerBuy {
            trainer,
            spell: mapping.spell,
        };
        assert!(validate_command(&Snapshot::from_state(&state), &command).is_ok());
        let action = ProposedAction {
            id: ActionId(31),
            task: TaskId(32),
            origin: PlanOrigin::SystemPolicy,
            stamp: base_stamp(),
            command: command.clone(),
        };
        let disabled = ActionValidator::validate(
            &Snapshot::from_state(&state),
            ValidationContext {
                current: base_stamp(),
                stage: ActivationStage::Maintain,
                permissions: PermissionSet::MAINTENANCE,
                bank_keep_item_ids: Vec::new(),
                auto_professions_enabled: false,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
            },
            action,
        );
        assert!(matches!(
            disabled,
            ValidationOutcome::Rejected(ActionFailure { code, .. })
                if code == "profession_training_disabled"
        ));
        state.trainer.offers[0].spell = u32::MAX;
        assert!(validate_command(&Snapshot::from_state(&state), &command).is_err());
        state.trainer.offers[0].spell = mapping.spell;
        state.entities.0.get_mut(&trainer).unwrap().npc_flags = Some(0x40);
        assert!(validate_command(&Snapshot::from_state(&state), &command).is_err());
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
                bank_keep_item_ids: Vec::new(),
                auto_professions_enabled: true,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
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
                    bank_keep_item_ids: Vec::new(),
                    auto_professions_enabled: true,
                    battleground_pvp_authorized: false,
                    fresh_group_loot_rolls: Vec::new(),
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
    fn vendor_buy_requires_matching_safe_offer_and_preserves_money_reserve() {
        let vendor = EntityId(55);
        let player_position = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(4);
        state.position.player = Some(player_position);
        state.inventory.vendor = Some(vendor);
        state.inventory.money = 1_120;
        state.inventory.vendor_inventory = Some(wow_state::inventory::VendorInventory {
            vendor,
            offers: vec![wow_state::inventory::VendorOffer {
                slot: 3,
                item: 6947,
                stock: Some(5),
                price_copper: 120,
                buy_count: 5,
                extended_cost: 0,
            }],
        });
        state.inventory.item_metadata.insert(
            6947,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Instant Poison IX".into(),
                item_class: 0,
                subclass: 6,
                allowable_class: 1 << 3,
                required_level: 60,
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(7),
            wow_state::entities::EntityState {
                id: EntityId(7),
                level: Some(80),
                ..Default::default()
            },
        );
        let seller_entry = wow_infra::world_knowledge::embedded_azerothcore_catalog()
            .world()
            .vendor_services
            .iter()
            .find(|service| service.can_sell)
            .expect("catalog has a seller")
            .entry_id;
        state.entities.0.insert(
            vendor,
            wow_state::entities::EntityState {
                id: vendor,
                entry: seller_entry,
                kind: wow_state::entities::EntityKind::Unit,
                interactable: true,
                position: Some(player_position),
                ..Default::default()
            },
        );
        let command = GameplayCommand::VendorBuy {
            vendor,
            item: 6947,
            slot: 3,
            count: 1,
        };
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_ok());

        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].item = 6948;
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_err());
        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].item = 6947;

        let oversized = GameplayCommand::VendorBuy {
            vendor,
            item: 6947,
            slot: 3,
            count: wow_domain::MAX_MAINTENANCE_VENDOR_BUY_LOTS + 1,
        };
        assert!(validate_economy_command(&Snapshot::from_state(&state), &oversized).is_err());

        state.inventory.money = 1_119;
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_err());
        state.inventory.money = 2_000;
        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].stock = Some(4);
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_err());

        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].stock = Some(5);
        state.capabilities.class_id = Some(1);
        state.inventory.item_metadata.insert(
            6947,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Linen Bandage".into(),
                item_class: 0,
                subclass: 7,
                allowable_class: 0,
                required_level: 1,
                use_spell_id: 102,
                ..Default::default()
            },
        );
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_ok());

        state.inventory.item_metadata.insert(
            6947,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Mana Potion".into(),
                item_class: 0,
                subclass: 1,
                allowable_class: 0,
                required_level: 1,
                use_spell_id: 103,
                ..Default::default()
            },
        );
        assert!(
            validate_economy_command(&Snapshot::from_state(&state), &command).is_ok(),
            "class and level eligible potion templates are valid maintenance offers"
        );

        state.inventory.item_metadata.insert(
            6947,
            wow_state::inventory::ItemTemplateMetadata {
                name: "Unrelated Consumable".into(),
                item_class: 0,
                subclass: 0,
                use_spell_id: 102,
                ..Default::default()
            },
        );
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_err());

        state.capabilities.class_id = Some(8);
        state.inventory.money = 2_000;
        state.inventory.free_slots = 1;
        state.inventory.instances_authoritative = true;
        state.inventory.equipment_slots_authoritative = true;
        state.inventory.item_metadata.insert(
            6947,
            wow_state::inventory::ItemTemplateMetadata {
                item_class: 4,
                inventory_type: 1,
                quality: 1,
                item_level: 10,
                allowable_class: 0,
                required_level: 1,
                ..Default::default()
            },
        );
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_ok());
        state.inventory.vendor_inventory.as_mut().unwrap().offers[0].price_copper = 501;
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_err());
    }

    #[test]
    fn ammo_selection_requires_authoritative_compatible_bag_evidence() {
        let mut state = AuthoritativeState::default();
        state.revision = StateRevision(7);
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(3);
        state.inventory.equipment_slots_authoritative = true;
        state.inventory.instances_authoritative = true;
        state.inventory.equipped_items.insert(17, 10_001);
        state.inventory.item_metadata.insert(
            10_001,
            wow_state::inventory::ItemTemplateMetadata {
                item_class: 2,
                ammo_type: 2,
                delay_ms: 3_000,
                ..Default::default()
            },
        );
        state.inventory.item_metadata.insert(
            2512,
            wow_state::inventory::ItemTemplateMetadata {
                item_class: 6,
                subclass: 2,
                required_level: 1,
                ..Default::default()
            },
        );
        state.inventory.instances.insert(
            EntityId(90),
            wow_state::inventory::InventoryItemInstance {
                item: 2512,
                guid: EntityId(90),
                backpack_slot: 23,
                count: 20,
            },
        );
        state.entities.0.insert(
            EntityId(7),
            EntityState {
                level: Some(20),
                ..Default::default()
            },
        );
        let command = GameplayCommand::SetAmmo { item: 2512 };
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_ok());

        state.inventory.instances.remove(&EntityId(90));
        assert!(validate_economy_command(&Snapshot::from_state(&state), &command).is_err());
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
                bank_keep_item_ids: Vec::new(),
                auto_professions_enabled: true,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
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
                bank_keep_item_ids: Vec::new(),
                auto_professions_enabled: true,
                battleground_pvp_authorized: false,
                fresh_group_loot_rolls: Vec::new(),
                permissions: PermissionSet::ALL,
            },
            action,
        );
        assert!(
            matches!(outcome, ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "origin_forbidden")
        );
    }

    #[test]
    fn travel_cleanup_commands_are_reserved_for_policy_or_operator_actions() {
        assert!(origin_authorized(
            PlanOrigin::SystemPolicy,
            &GameplayCommand::CancelMount
        ));
        assert!(origin_authorized(
            PlanOrigin::Operator,
            &GameplayCommand::CancelAura { spell: 783 }
        ));
        assert!(!origin_authorized(
            PlanOrigin::Llm,
            &GameplayCommand::CancelAura { spell: 783 }
        ));
        assert!(!origin_authorized(
            PlanOrigin::Recovery,
            &GameplayCommand::CancelMount
        ));

        let system_action = ProposedAction {
            id: ActionId(1),
            task: TaskId(1),
            origin: PlanOrigin::SystemPolicy,
            stamp: base_stamp(),
            command: GameplayCommand::CancelMount,
        };
        let context = ValidationContext {
            current: base_stamp(),
            stage: ActivationStage::Act,
            bank_keep_item_ids: Vec::new(),
            auto_professions_enabled: true,
            battleground_pvp_authorized: false,
            fresh_group_loot_rolls: Vec::new(),
            permissions: PermissionSet::MOVE,
        };
        assert!(matches!(
            ActionValidator::validate(
                &snapshot(),
                ValidationContext {
                    current: base_stamp(),
                    stage: ActivationStage::Act,
                    bank_keep_item_ids: Vec::new(),
                    auto_professions_enabled: true,
                    battleground_pvp_authorized: false,
                    fresh_group_loot_rolls: Vec::new(),
                    permissions: PermissionSet::MOVE,
                },
                system_action,
            ),
            ValidationOutcome::Sendable(_)
        ));

        let llm_action = ProposedAction {
            id: ActionId(2),
            task: TaskId(2),
            origin: PlanOrigin::Llm,
            stamp: base_stamp(),
            command: GameplayCommand::CancelAura { spell: 783 },
        };
        assert!(matches!(
            ActionValidator::validate(&snapshot(), context, llm_action),
            ValidationOutcome::Rejected(ActionFailure { code, .. }) if code == "origin_forbidden"
        ));
    }
}
