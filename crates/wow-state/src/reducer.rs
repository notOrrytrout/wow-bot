use crate::{AuthoritativeState, ProtocolObservation, StateDelta};
use wow_domain::EntityId;

fn ensure_quest_giver(state: &mut AuthoritativeState, giver: EntityId) {
    state
        .entities
        .0
        .entry(giver)
        .or_insert_with(|| crate::entities::EntityState {
            id: giver,
            interactable: true,
            ..Default::default()
        });
}

fn advance_loot_generation(state: &mut AuthoritativeState, ownership: crate::LootOwnership) {
    state.inventory.loot_generation = state.inventory.loot_generation.wrapping_add(1);
    if matches!(ownership, crate::LootOwnership::Bot) {
        state.inventory.bot_loot_generation = state.inventory.bot_loot_generation.wrapping_add(1);
    }
}

fn record_quest_completion(state: &mut AuthoritativeState, quest: u32) {
    if !state.quests.completed.contains(&quest) {
        state.quests.completed.push(quest);
    }
}

fn mark_entity_changed(delta: &mut StateDelta, entity: EntityId, sections: &[&str]) {
    delta.touched_entities.push(entity);
    delta
        .changed
        .extend(sections.iter().map(|section| (*section).into()));
}

fn clear_world_transients(state: &mut AuthoritativeState) {
    state.entities = Default::default();
    state.auras = Default::default();
    state.transport = Default::default();
    state.control = Default::default();
    state.pet = Default::default();
    state.trainer = Default::default();
    state.active_casts.clear();
    state.quests.giver_status.clear();
    state.quests.offers.clear();
    state.quests.turn_in.clear();
    state.inventory.current_loot = None;
    state.inventory.current_loot_owner = None;
    state.inventory.vendor = None;
    state.inventory.vendor_inventory = None;
    state.inventory.equipment_condition = Default::default();
    state.inventory.trade = Default::default();
    state.inventory.auction = Default::default();
    state.inventory.mailbox = Default::default();
}

pub fn reduce(state: &mut AuthoritativeState, observation: ProtocolObservation) -> StateDelta {
    let mut delta = StateDelta::default();
    match observation {
        ProtocolObservation::Authenticated { realm } => {
            state.session.authenticated = true;
            state.session.realm = realm;
            delta.changed.push("session".into());
        }
        ProtocolObservation::EnteredWorld {
            character_guid,
            position,
        } => {
            if state.session.in_world {
                clear_world_transients(state);
                delta.changed.extend(
                    [
                        "entities",
                        "auras",
                        "transport",
                        "control",
                        "pet",
                        "active_casts",
                        "quests",
                        "inventory",
                        "trainer",
                    ]
                    .map(str::to_owned),
                );
            }
            state.session.in_world = true;
            state.session.character_guid = Some(character_guid);
            state.position.player = position;
            delta.changed.extend(["session".into(), "position".into()]);
        }
        ProtocolObservation::WorldChanged {
            character_guid,
            position,
        } => {
            state.session.in_world = true;
            state.session.character_guid = Some(character_guid);
            state.position.player = Some(position);
            state.position.moving = false;
            state.position.flags = 0;
            state.position.client_time = 0;
            clear_world_transients(state);
            delta.changed.extend(
                [
                    "session",
                    "position",
                    "entities",
                    "auras",
                    "transport",
                    "control",
                    "pet",
                    "active_casts",
                    "quests",
                    "inventory",
                    "trainer",
                ]
                .map(str::to_owned),
            );
        }
        ProtocolObservation::LeftWorld => {
            let revision = state.revision;
            *state = AuthoritativeState::default();
            state.revision = revision;
            delta.changed.extend(
                [
                    "session",
                    "position",
                    "entities",
                    "inventory",
                    "life",
                    "quests",
                    "professions",
                    "trainer",
                    "group",
                    "capabilities",
                    "control",
                    "auras",
                    "desync",
                ]
                .map(str::to_owned),
            );
        }
        ProtocolObservation::PlayerPosition {
            position,
            moving,
            flags,
            client_time,
        } => {
            state.position.player = Some(position);
            state.position.moving = moving;
            state.position.flags = flags;
            state.position.client_time = client_time;
            delta.changed.push("position".into());
        }
        ProtocolObservation::RunSpeedChanged { yards_per_second } => {
            if yards_per_second.is_finite() && (0.1..=100.0).contains(&yards_per_second) {
                state.position.run_speed_yards_per_second = Some(yards_per_second);
                delta.changed.push("position".into());
            }
        }
        ProtocolObservation::ControlledMover {
            mover,
            position,
            flags,
        } => {
            state.control.mover = mover;
            state.control.mover_position = position;
            state.control.movement_flags = flags;
            if mover.is_none() {
                state.control.abilities.clear();
            }
            delta.changed.push("control".into());
        }
        ProtocolObservation::ControlledAbilities { mover, spells } => {
            if state.control.mover == Some(mover) {
                state.control.abilities = spells.into_iter().collect();
                delta.changed.push("control".into());
            }
        }
        ProtocolObservation::Transport { state: transport } => {
            if state.transport != transport {
                state.transport = transport;
                delta.changed.push("transport".into());
            }
        }
        ProtocolObservation::PetControl {
            pet,
            reaction,
            abilities,
        } => {
            state.pet.control_known = true;
            state.pet.guid = pet;
            state.pet.reaction = reaction;
            state.pet.abilities = abilities;
            delta.changed.push("pet".into());
        }
        ProtocolObservation::CastFailed { .. } => {}
        ProtocolObservation::CastStarted {
            caster,
            spell,
            started_at_ms,
            ends_at_ms,
        } => {
            if spell != 0 && ends_at_ms > started_at_ms {
                state.active_casts.insert(
                    caster,
                    crate::ActiveCastState {
                        spell,
                        started_at_ms,
                        ends_at_ms,
                    },
                );
                mark_entity_changed(&mut delta, caster, &["active_casts"]);
            }
        }
        ProtocolObservation::CastFinished { caster, spell } => {
            let matches_spell = spell == 0
                || state
                    .active_casts
                    .get(&caster)
                    .is_some_and(|active| active.spell == spell);
            if matches_spell && state.active_casts.remove(&caster).is_some() {
                mark_entity_changed(&mut delta, caster, &["active_casts"]);
            }
        }
        ProtocolObservation::CastUpdated { caster, ends_at_ms } => {
            if let Some(active) = state.active_casts.get_mut(&caster) {
                active.ends_at_ms = ends_at_ms;
                if ends_at_ms == 0 {
                    state.active_casts.remove(&caster);
                }
                mark_entity_changed(&mut delta, caster, &["active_casts"]);
            }
        }
        ProtocolObservation::CorpseLocation { position } => {
            state.life.corpse = position;
            state.life.recovery_generation = state.life.recovery_generation.wrapping_add(1);
            delta.changed.push("life".into());
        }
        ProtocolObservation::CorpseReclaimDelay { ready_at_ms } => {
            state.life.reclaim_ready_at_ms = ready_at_ms;
            delta.changed.push("life".into());
        }
        ProtocolObservation::EntityUpsert { entity } => {
            let entity_id = entity.id;
            let player_leveled = state.session.character_guid == Some(entity_id.0)
                && state
                    .entities
                    .0
                    .get(&entity_id)
                    .and_then(|old| old.level)
                    .zip(entity.level)
                    .is_some_and(|(old, new)| new > old);
            state.entities.0.insert(entity_id, entity);
            if player_leveled {
                state.trainer = Default::default();
                delta.changed.push("trainer".into());
            }
            mark_entity_changed(&mut delta, entity_id, &["entities"]);
        }
        ProtocolObservation::EntityRemoved { entity } => {
            state.entities.0.remove(&entity);
            state.active_casts.remove(&entity);
            if state.inventory.vendor == Some(entity) {
                state.inventory.vendor = None;
                state.inventory.vendor_inventory = None;
            }
            if state.inventory.current_loot == Some(entity) {
                state.inventory.current_loot = None;
                state.inventory.current_loot_owner = None;
                state.inventory.loot_generation = state.inventory.loot_generation.wrapping_add(1);
            }
            if state.inventory.trade.partner == Some(entity) {
                state.inventory.trade.open = false;
                state.inventory.trade.generation = state.inventory.trade.generation.wrapping_add(1);
            }
            let trainer_removed = state.trainer.trainer == Some(entity);
            if trainer_removed {
                state.trainer = Default::default();
            }
            if trainer_removed {
                mark_entity_changed(&mut delta, entity, &["entities", "inventory", "trainer"]);
            } else {
                mark_entity_changed(&mut delta, entity, &["entities", "inventory"]);
            }
        }
        ProtocolObservation::CreatureKilled { victim, .. } => {
            if let Some(entity) = state.entities.0.get_mut(&victim) {
                entity.mark_dead();
                mark_entity_changed(&mut delta, victim, &["entities"]);
            }
        }
        ProtocolObservation::InventoryCount { item, count } => {
            if count == 0 {
                state.inventory.items.remove(&item);
            } else {
                state.inventory.items.insert(item, count);
            }
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::ItemTemplate { item, metadata } => {
            state.inventory.item_metadata.insert(item, metadata);
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::InventoryInstances { items } => {
            state.inventory.instances = items
                .into_iter()
                .map(|instance| (instance.guid, instance))
                .collect();
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::EquipmentCondition(condition) => {
            state.inventory.equipment_condition = condition;
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::InventoryFreeSlots { count } => {
            state.inventory.free_slots = count;
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::EquippedRangedItem { item } => {
            state.inventory.equipped_ranged_item = item;
            state.inventory.equipment_authoritative = true;
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::EquippedItems { items, instances } => {
            state.inventory.equipment_slots_authoritative = items.is_some();
            state.inventory.equipped_items = items.unwrap_or_default();
            state.inventory.equipped_item_instances = instances.unwrap_or_default();
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::Money { copper } => {
            state.inventory.money = copper;
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::LootOpened { target, ownership } => {
            state.inventory.current_loot = Some(target);
            state.inventory.current_loot_owner = Some(ownership);
            advance_loot_generation(state, ownership);
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::LootRejected { .. } => {}
        ProtocolObservation::LootClosed { ownership } => {
            state.inventory.current_loot = None;
            state.inventory.current_loot_owner = None;
            advance_loot_generation(state, ownership);
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::VendorOpened { vendor } => {
            if state.inventory.vendor != Some(vendor) {
                state.inventory.vendor_inventory = None;
            }
            state.inventory.vendor = Some(vendor);
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::VendorInventory { vendor, offers } => {
            state.inventory.vendor = Some(vendor);
            state.inventory.vendor_inventory =
                Some(crate::inventory::VendorInventory { vendor, offers });
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::VendorStockUpdated {
            vendor,
            slot,
            stock,
            purchased_lots: _,
        } => {
            if let Some(inventory) = state
                .inventory
                .vendor_inventory
                .as_mut()
                .filter(|inventory| inventory.vendor == vendor)
                && let Some(offer) = inventory.offers.iter_mut().find(|offer| offer.slot == slot)
            {
                offer.stock = stock;
                delta.changed.push("inventory".into());
            }
        }
        ProtocolObservation::VendorClosed => {
            state.inventory.vendor = None;
            state.inventory.vendor_inventory = None;
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::TrainerList {
            trainer,
            trainer_type,
            offers,
        } => {
            state.trainer = crate::trainer::TrainerState {
                trainer: Some(trainer),
                trainer_type: Some(trainer_type),
                offers,
            };
            delta.changed.push("trainer".into());
        }
        ProtocolObservation::Trade(trade) => {
            state.inventory.trade = trade;
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::Auction(auction) => {
            state.inventory.auction = auction;
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::Mailbox(mailbox) => {
            let generation = state.inventory.mailbox.generation.saturating_add(1);
            state.inventory.mailbox = mailbox;
            state.inventory.mailbox.generation = generation;
            state.inventory.mailbox.authoritative = true;
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::QuestGiverStatus { giver, status } => {
            state.quests.giver_status.insert(giver, status);
            if !matches!(status, 2 | 4 | 7 | 8) {
                state.quests.offers.retain(|_, offer| offer.giver != giver);
            }
            ensure_quest_giver(state, giver);
            mark_entity_changed(&mut delta, giver, &["quests", "entities"]);
        }
        ProtocolObservation::QuestGiverListReceived { giver, .. } => {
            state.quests.offers.retain(|_, offer| offer.giver != giver);
        }
        ProtocolObservation::QuestOffer { giver, quest, icon } => {
            if !state.quests.active.contains_key(&quest) && !state.quests.completed.contains(&quest)
            {
                state
                    .quests
                    .offers
                    .insert(quest, crate::quests::QuestOffer { giver, icon });
                ensure_quest_giver(state, giver);
                mark_entity_changed(&mut delta, giver, &["quests", "entities"]);
            }
        }
        ProtocolObservation::QuestAccepted { quest } => {
            state.quests.active.entry(quest).or_default();
            state.quests.offers.remove(&quest);
            delta.changed.push("quests".into());
        }
        ProtocolObservation::QuestDefinition { definition } => {
            state
                .quests
                .definitions
                .insert(definition.quest, definition);
            delta.changed.push("quests".into());
        }
        ProtocolObservation::QuestTurnInDialog { quest, dialog } => {
            state.quests.turn_in.insert(quest, dialog);
            delta.changed.push("quests".into());
        }
        ProtocolObservation::QuestProgress {
            quest,
            objectives,
            complete,
        } => {
            state.quests.offers.remove(&quest);
            state.quests.active.insert(
                quest,
                crate::quests::QuestProgress {
                    complete,
                    objectives,
                },
            );
            delta.changed.push("quests".into());
        }
        ProtocolObservation::QuestRemoved { quest } => {
            let was_turning_in = state.quests.turn_in.contains_key(&quest);
            let was_complete = state
                .quests
                .active
                .get(&quest)
                .is_some_and(|progress| progress.complete);
            state.quests.active.remove(&quest);
            state.quests.turn_in.remove(&quest);
            if was_turning_in && was_complete {
                record_quest_completion(state, quest);
            }
            delta.changed.push("quests".into());
        }
        ProtocolObservation::QuestCompleted { quest } => {
            state.quests.active.remove(&quest);
            record_quest_completion(state, quest);
            delta.changed.push("quests".into());
        }
        ProtocolObservation::SpellKnown { spell } => {
            state.capabilities.spells.insert(spell);
            delta.changed.push("capabilities".into());
        }
        ProtocolObservation::SpellCooldown { spell, ready_at_ms } => {
            state
                .capabilities
                .spell_cooldowns
                .insert(spell, ready_at_ms);
            delta.changed.push("capabilities".into());
        }
        ProtocolObservation::SpellGlobalCooldown {
            spell,
            started_at_ms,
        } => {
            state.capabilities.global_cooldown_spell = Some(spell);
            state.capabilities.global_cooldown_started_at_ms = Some(started_at_ms);
            delta.changed.push("capabilities".into());
        }
        ProtocolObservation::PlayerRunes { runes } => {
            state.capabilities.runes = runes;
            delta.changed.push("capabilities".into());
        }
        ProtocolObservation::ComboPoints { target, points } => {
            match (target, points) {
                (Some(target), Some(points)) => {
                    state.capabilities.combo_points.insert(target, points);
                }
                (Some(target), None) => {
                    state.capabilities.combo_points.remove(&target);
                }
                (None, _) => state.capabilities.combo_points.clear(),
            }
            delta.changed.push("capabilities".into());
        }
        ProtocolObservation::PlayerClass { class_id } => {
            state.capabilities.class_id = Some(class_id);
            state.capabilities.specialization_tree =
                crate::talents::active_tree(Some(class_id), &state.capabilities.active_talents);
            delta
                .changed
                .extend(["capabilities".into(), "specialization".into()]);
        }
        ProtocolObservation::PlayerTalents {
            group_count,
            active_group,
            talents,
            glyph_properties,
        } => {
            let valid = group_count.is_some_and(|count| (1..=2).contains(&count))
                && active_group.is_some_and(|group| group_count.is_some_and(|count| group < count));
            state.capabilities.talent_group_count = valid.then_some(group_count).flatten();
            state.capabilities.active_talent_group = valid.then_some(active_group).flatten();
            state.capabilities.active_talents = if valid { talents } else { Vec::new() };
            state.capabilities.active_glyph_properties =
                if valid { glyph_properties } else { None };
            state.capabilities.specialization_tree = valid
                .then(|| {
                    crate::talents::active_tree(
                        state.capabilities.class_id,
                        &state.capabilities.active_talents,
                    )
                })
                .flatten();
            delta
                .changed
                .extend(["capabilities".into(), "specialization".into()]);
        }
        ProtocolObservation::AuraSnapshot { entity, auras } => {
            state.auras.by_entity.insert(
                entity,
                auras.into_iter().map(|aura| (aura.slot, aura)).collect(),
            );
            delta.changed.push("auras".into());
        }
        ProtocolObservation::AuraSlot { entity, slot, aura } => {
            let slots = state.auras.by_entity.entry(entity).or_default();
            if let Some(aura) = aura {
                let aura = if let Some(previous) =
                    slots.get(&slot).filter(|old| old.spell == aura.spell)
                {
                    crate::auras::AuraInstance {
                        positive: aura.positive.or(previous.positive),
                        caster: aura.caster.or(previous.caster),
                        max_duration_ms: aura.max_duration_ms.or(previous.max_duration_ms),
                        remaining_ms: aura.remaining_ms.or(previous.remaining_ms),
                        observed_at_ms: aura.observed_at_ms.or(previous.observed_at_ms),
                        ..aura
                    }
                } else {
                    aura
                };
                slots.insert(slot, aura);
            } else {
                slots.remove(&slot);
            }
            delta.changed.push("auras".into());
        }
        ProtocolObservation::Skill {
            skill,
            current,
            max,
        } => {
            state.professions.known = true;
            state.professions.skills.insert(skill, (current, max));
            delta.changed.push("professions".into());
        }
        ProtocolObservation::ProfessionSnapshot { skills, slots } => {
            state.professions.known = true;
            state.professions.skills = skills;
            state.professions.slots = slots;
            delta.changed.push("professions".into());
        }
        ProtocolObservation::RecipeKnown { recipe } => {
            state.professions.known_recipes.insert(recipe);
            delta.changed.push("professions".into());
        }
        ProtocolObservation::ProfessionFlags {
            cooking,
            first_aid,
            fishing,
        } => {
            state.professions.cooking = cooking;
            state.professions.first_aid = first_aid;
            state.professions.fishing = fishing;
            state.capabilities.can_fish = fishing;
            delta
                .changed
                .extend(["professions".into(), "capabilities".into()]);
        }
        ProtocolObservation::Group(group) => {
            state.group = group;
            delta.changed.push("group".into());
        }
        ProtocolObservation::Desync { reason } => {
            state.desync.suspect = true;
            state.desync.reasons.push(reason);
            delta.changed.push("desync".into());
        }
        ProtocolObservation::Resynchronized => {
            state.desync = Default::default();
            delta.changed.push("desync".into());
        }
        ProtocolObservation::Raw { .. } => {}
    }
    state.revision = state.revision.next();
    delta.revision = state.revision;
    delta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::EntityState,
        inventory::InventoryItemInstance,
        quests::{QuestProgress, QuestTurnInDialog, QuestTurnInStage},
    };
    use std::collections::BTreeMap;
    use wow_domain::{EntityId, Vec3, WorldPosition};

    #[test]
    fn partial_aura_update_keeps_known_caster_and_server_duration() {
        let mut state = AuthoritativeState::default();
        let target = EntityId(22);
        reduce(
            &mut state,
            ProtocolObservation::AuraSlot {
                entity: target,
                slot: 1,
                aura: Some(crate::auras::AuraInstance {
                    slot: 1,
                    spell: 172,
                    positive: Some(false),
                    caster: Some(EntityId(7)),
                    max_duration_ms: Some(18_000),
                    remaining_ms: Some(12_000),
                    observed_at_ms: Some(100),
                }),
            },
        );
        reduce(
            &mut state,
            ProtocolObservation::AuraSlot {
                entity: target,
                slot: 1,
                aura: Some(crate::auras::AuraInstance {
                    slot: 1,
                    spell: 172,
                    positive: None,
                    caster: None,
                    max_duration_ms: None,
                    remaining_ms: None,
                    observed_at_ms: None,
                }),
            },
        );

        let aura = &state.auras.by_entity[&target][&1];
        assert_eq!(aura.caster, Some(EntityId(7)));
        assert_eq!(aura.remaining_at(1_100), Some(11_000));
    }

    #[test]
    fn authoritative_pet_and_profession_snapshots_keep_unknown_distinct_from_empty() {
        let mut state = AuthoritativeState::default();
        assert_eq!(state.pet.has_active_pet(&state.entities), None);
        reduce(
            &mut state,
            ProtocolObservation::PetControl {
                pet: None,
                reaction: None,
                abilities: Vec::new(),
            },
        );
        assert_eq!(state.pet.has_active_pet(&state.entities), Some(false));

        reduce(
            &mut state,
            ProtocolObservation::PetControl {
                pet: Some(EntityId(10)),
                reaction: None,
                abilities: Vec::new(),
            },
        );
        assert_eq!(state.pet.has_active_pet(&state.entities), Some(true));
        reduce(
            &mut state,
            ProtocolObservation::EntityUpsert {
                entity: EntityState {
                    id: EntityId(10),
                    health: Some((0, 100)),
                    ..Default::default()
                },
            },
        );
        assert_eq!(state.pet.has_active_pet(&state.entities), Some(false));

        reduce(
            &mut state,
            ProtocolObservation::ProfessionSnapshot {
                skills: [(164, (75, 150))].into_iter().collect(),
                slots: [(0, 164)].into_iter().collect(),
            },
        );
        assert!(state.professions.known);
        assert_eq!(state.professions.skill(164), 75);
        assert_eq!(state.professions.slots.get(&0), Some(&164));
        let sanitized = crate::SanitizedSnapshot::from(&crate::Snapshot::from_state(&state));
        assert_eq!(sanitized.has_active_pet, Some(false));
        assert_eq!(sanitized.profession_skills.get(&164), Some(&(75, 150)));
    }

    #[test]
    fn quest_completion_events_share_unique_recording() {
        let mut state = AuthoritativeState::default();
        state.quests.active.insert(
            42,
            QuestProgress {
                complete: true,
                objectives: vec![1],
            },
        );
        state.quests.turn_in.insert(
            42,
            QuestTurnInDialog {
                giver: EntityId(7),
                stage: QuestTurnInStage::RequestItems { can_complete: true },
            },
        );

        reduce(&mut state, ProtocolObservation::QuestRemoved { quest: 42 });
        reduce(
            &mut state,
            ProtocolObservation::QuestCompleted { quest: 42 },
        );

        assert_eq!(state.quests.completed, vec![42]);
    }

    #[test]
    fn quest_giver_list_does_not_reintroduce_active_or_completed_offers() {
        let mut state = AuthoritativeState::default();
        state.quests.active.insert(41, QuestProgress::default());
        state.quests.completed.push(42);

        reduce(
            &mut state,
            ProtocolObservation::QuestGiverListReceived {
                giver: EntityId(7),
                offer_count: 3,
            },
        );
        for quest in [41, 42, 43] {
            reduce(
                &mut state,
                ProtocolObservation::QuestOffer {
                    giver: EntityId(7),
                    quest,
                    icon: 0,
                },
            );
        }

        assert_eq!(state.quests.offers.len(), 1);
        assert_eq!(
            state.quests.offers.get(&43).map(|offer| offer.giver),
            Some(EntityId(7))
        );
    }

    #[test]
    fn mailbox_observations_advance_generation_and_keep_cod_evidence() {
        let mut state = AuthoritativeState::default();
        let mut mails = BTreeMap::new();
        mails.insert(
            77,
            crate::inventory::MailEntry {
                mail_id: 77,
                money: 123,
                cod_copper: Some(0),
                attachments: BTreeMap::new(),
            },
        );
        let observed = || {
            ProtocolObservation::Mailbox(crate::inventory::MailboxState {
                generation: 0,
                authoritative: true,
                mails: mails.clone(),
            })
        };
        reduce(&mut state, observed());
        assert_eq!(state.inventory.mailbox.generation, 1);
        assert!(state.inventory.mailbox.authoritative);
        assert_eq!(state.inventory.mailbox.mails[&77].cod_copper, Some(0));

        reduce(&mut state, observed());
        assert_eq!(state.inventory.mailbox.generation, 2);
    }

    #[test]
    fn world_change_clears_old_world_objects_and_keeps_character_progress() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.position.player = Some(WorldPosition {
            map: 1,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 0.0,
        });
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                ..Default::default()
            },
        );
        state
            .auras
            .by_entity
            .insert(EntityId(7), Default::default());
        state.active_casts.insert(
            EntityId(7),
            crate::ActiveCastState {
                spell: 686,
                started_at_ms: 1,
                ends_at_ms: 2,
            },
        );
        state.quests.active.insert(
            42,
            QuestProgress {
                complete: false,
                objectives: vec![1],
            },
        );
        state.inventory.items.insert(99, 2);
        state.capabilities.spells.insert(686);
        state.quests.giver_status.insert(EntityId(9), 1);

        reduce(
            &mut state,
            ProtocolObservation::WorldChanged {
                character_guid: 7,
                position: WorldPosition {
                    map: 2,
                    point: Vec3::new(10.0, 20.0, 30.0),
                    orientation: 1.0,
                },
            },
        );

        assert_eq!(state.position.player.map(|position| position.map), Some(2));
        assert!(state.entities.0.is_empty());
        assert!(state.auras.by_entity.is_empty());
        assert!(state.active_casts.is_empty());
        assert!(state.quests.giver_status.is_empty());
        assert!(state.quests.active.contains_key(&42));
        assert_eq!(state.inventory.count(99), 2);
        assert!(state.capabilities.spells.contains(&686));
    }

    #[test]
    fn repeated_world_entry_clears_old_session_npcs() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.entities.0.insert(
            EntityId(99),
            EntityState {
                id: EntityId(99),
                ..Default::default()
            },
        );
        state.quests.active.insert(42, QuestProgress::default());

        reduce(
            &mut state,
            ProtocolObservation::EnteredWorld {
                character_guid: 7,
                position: Some(WorldPosition {
                    map: 2,
                    point: Vec3::new(10.0, 20.0, 30.0),
                    orientation: 1.0,
                }),
            },
        );

        assert!(state.entities.0.is_empty());
        assert!(state.quests.active.contains_key(&42));
    }

    #[test]
    fn leaving_world_removes_session_evidence_before_reentry() {
        let mut state = AuthoritativeState::default();
        let old_position = WorldPosition {
            map: 1,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 0.0,
        };
        reduce(
            &mut state,
            ProtocolObservation::Authenticated {
                realm: Some("old".into()),
            },
        );
        reduce(
            &mut state,
            ProtocolObservation::EnteredWorld {
                character_guid: 7,
                position: Some(old_position),
            },
        );
        reduce(
            &mut state,
            ProtocolObservation::EntityUpsert {
                entity: EntityState {
                    id: EntityId(9),
                    ..Default::default()
                },
            },
        );
        reduce(
            &mut state,
            ProtocolObservation::QuestProgress {
                quest: 42,
                objectives: vec![1],
                complete: false,
            },
        );
        reduce(
            &mut state,
            ProtocolObservation::InventoryCount { item: 99, count: 2 },
        );
        reduce(&mut state, ProtocolObservation::SpellKnown { spell: 123 });
        let before = state.revision;

        reduce(&mut state, ProtocolObservation::LeftWorld);
        assert_eq!(state.revision, before.next());
        assert!(!state.session.in_world);
        assert!(!state.session.authenticated);
        assert_eq!(state.session.character_guid, None);
        assert_eq!(state.position.player, None);
        assert!(state.entities.0.is_empty());
        assert!(state.quests.active.is_empty());
        assert!(state.inventory.items.is_empty());
        assert!(state.capabilities.spells.is_empty());

        reduce(
            &mut state,
            ProtocolObservation::EnteredWorld {
                character_guid: 8,
                position: None,
            },
        );
        assert!(state.session.in_world);
        assert_eq!(state.session.character_guid, Some(8));
        assert_eq!(state.position.player, None);
        assert!(state.entities.0.is_empty());
        assert!(state.quests.active.is_empty());
    }

    #[test]
    fn transport_observation_updates_and_clears_authoritative_state() {
        let mut state = AuthoritativeState::default();
        let attached = crate::transport::TransportState {
            attached: Some(true),
            transport: Some(EntityId(55)),
            relative_position: Some(Vec3::new(1.0, 2.0, 0.5)),
            relative_orientation: Some(0.25),
            transport_time: Some(300),
        };
        let delta = reduce(
            &mut state,
            ProtocolObservation::Transport {
                state: attached.clone(),
            },
        );
        assert_eq!(state.transport, attached);
        assert!(delta.changed.contains(&"transport".to_owned()));
        assert_eq!(
            reduce(
                &mut state,
                ProtocolObservation::Transport {
                    state: crate::transport::TransportState {
                        attached: Some(false),
                        ..Default::default()
                    },
                },
            )
            .changed,
            vec!["transport"]
        );
        assert_eq!(state.transport.attached, Some(false));
        assert_eq!(state.transport.transport, None);
    }

    #[test]
    fn inventory_snapshot_retains_each_stack_of_the_same_item() {
        let mut state = AuthoritativeState::default();
        let first = InventoryItemInstance {
            item: 99,
            guid: EntityId(10),
            backpack_slot: 3,
            count: 2,
        };
        let second = InventoryItemInstance {
            item: 99,
            guid: EntityId(11),
            backpack_slot: 1,
            count: 5,
        };
        reduce(
            &mut state,
            ProtocolObservation::InventoryInstances {
                items: vec![first, second.clone()],
            },
        );
        assert_eq!(state.inventory.instances.len(), 2);
        assert_eq!(state.inventory.usable_instance(99), Some(&second));
    }

    #[test]
    fn equipment_condition_is_authoritative_and_clears_on_world_reset() {
        let mut state = AuthoritativeState::default();
        let condition = crate::inventory::EquipmentCondition {
            observed: true,
            lowest_durability_percent: Some(0),
            broken_items: 1,
        };
        reduce(
            &mut state,
            ProtocolObservation::EquipmentCondition(condition),
        );
        assert_eq!(state.inventory.equipment_condition, condition);

        reduce(
            &mut state,
            ProtocolObservation::WorldChanged {
                character_guid: 1,
                position: WorldPosition {
                    map: 1,
                    point: Vec3::new(0.0, 0.0, 0.0),
                    orientation: 0.0,
                },
            },
        );
        assert_eq!(
            state.inventory.equipment_condition,
            crate::inventory::EquipmentCondition::default()
        );
    }

    #[test]
    fn trainer_offers_are_authoritative_and_clear_with_the_trainer_entity() {
        let mut state = AuthoritativeState::default();
        let trainer = EntityId(77);
        let offer = crate::trainer::TrainerSpellOffer {
            spell: 1234,
            usable: 0,
            cost_copper: 100,
            required_level: 10,
            required_skill_line: 0,
            required_skill_rank: 0,
        };
        reduce(
            &mut state,
            ProtocolObservation::TrainerList {
                trainer,
                trainer_type: 0,
                offers: vec![offer.clone()],
            },
        );
        assert_eq!(state.trainer.trainer, Some(trainer));
        assert_eq!(state.trainer.offers, vec![offer]);

        reduce(
            &mut state,
            ProtocolObservation::EntityRemoved { entity: trainer },
        );
        assert_eq!(state.trainer, crate::trainer::TrainerState::default());
    }

    #[test]
    fn trainer_offers_are_refreshed_after_an_authoritative_level_up() {
        let trainer = EntityId(77);
        let player = EntityId(7);
        let mut state = AuthoritativeState::default();
        state.session.character_guid = Some(player.0);
        state.entities.0.insert(
            player,
            crate::entities::EntityState {
                id: player,
                level: Some(10),
                ..Default::default()
            },
        );
        state.trainer = crate::trainer::TrainerState {
            trainer: Some(trainer),
            trainer_type: Some(0),
            offers: vec![crate::trainer::TrainerSpellOffer {
                spell: 1234,
                ..Default::default()
            }],
        };

        reduce(
            &mut state,
            ProtocolObservation::EntityUpsert {
                entity: crate::entities::EntityState {
                    id: player,
                    level: Some(11),
                    ..Default::default()
                },
            },
        );
        assert_eq!(state.trainer, crate::trainer::TrainerState::default());
    }

    #[test]
    fn vendor_offer_inventory_updates_only_from_matching_authoritative_vendor() {
        let mut state = AuthoritativeState::default();
        let vendor = EntityId(55);
        let offer = crate::inventory::VendorOffer {
            slot: 3,
            item: 6947,
            stock: Some(5),
            price_copper: 100,
            buy_count: 5,
            extended_cost: 0,
        };
        reduce(
            &mut state,
            ProtocolObservation::VendorInventory {
                vendor,
                offers: vec![offer.clone()],
            },
        );
        assert_eq!(state.inventory.vendor, Some(vendor));
        assert_eq!(
            state.inventory.vendor_inventory.as_ref().unwrap().offers,
            vec![offer]
        );

        reduce(
            &mut state,
            ProtocolObservation::VendorStockUpdated {
                vendor: EntityId(56),
                slot: 3,
                stock: Some(4),
                purchased_lots: 1,
            },
        );
        assert_eq!(
            state.inventory.vendor_inventory.as_ref().unwrap().offers[0].stock,
            Some(5)
        );
        reduce(
            &mut state,
            ProtocolObservation::VendorStockUpdated {
                vendor,
                slot: 3,
                stock: Some(4),
                purchased_lots: 1,
            },
        );
        assert_eq!(
            state.inventory.vendor_inventory.as_ref().unwrap().offers[0].stock,
            Some(4)
        );
        reduce(&mut state, ProtocolObservation::VendorClosed);
        assert!(state.inventory.vendor_inventory.is_none());
    }

    #[test]
    fn equipped_item_observation_tracks_guid_and_temporary_enchant_evidence() {
        let mut state = AuthoritativeState::default();
        let weapon = crate::inventory::EquippedItemInstance {
            item: 19019,
            guid: EntityId(0x1234),
            temporary_enchanted: Some(false),
        };
        reduce(
            &mut state,
            ProtocolObservation::EquippedItems {
                items: Some([(15, 19019)].into_iter().collect()),
                instances: Some([(15, weapon)].into_iter().collect()),
            },
        );
        assert_eq!(
            state.inventory.equipped_item_instances.get(&15),
            Some(&weapon)
        );
        assert_eq!(state.inventory.equipped_items.get(&15), Some(&19019));

        reduce(
            &mut state,
            ProtocolObservation::EquippedItems {
                items: Some(BTreeMap::new()),
                instances: Some(BTreeMap::new()),
            },
        );
        assert!(state.inventory.equipped_item_instances.is_empty());
    }
    #[test]
    fn removing_entity_clears_dependent_service_state() {
        let mut s = AuthoritativeState::default();
        let id = EntityId(7);
        s.entities.0.insert(
            id,
            EntityState {
                id,
                ..Default::default()
            },
        );
        s.inventory.vendor = Some(id);
        s.inventory.current_loot = Some(id);
        s.inventory.trade.partner = Some(id);
        s.inventory.trade.open = true;
        reduce(&mut s, ProtocolObservation::EntityRemoved { entity: id });
        assert!(!s.entities.0.contains_key(&id));
        assert_eq!(s.inventory.vendor, None);
        assert_eq!(s.inventory.current_loot, None);
        assert!(!s.inventory.trade.open);
    }
    #[test]
    fn player_loot_does_not_advance_bot_loot_generation() {
        let mut s = AuthoritativeState::default();
        let id = EntityId(9);
        reduce(
            &mut s,
            ProtocolObservation::LootOpened {
                target: id,
                ownership: crate::LootOwnership::Player,
            },
        );
        assert_eq!(s.inventory.loot_generation, 1);
        assert_eq!(s.inventory.bot_loot_generation, 0);
        reduce(
            &mut s,
            ProtocolObservation::LootClosed {
                ownership: crate::LootOwnership::Player,
            },
        );
        assert_eq!(s.inventory.loot_generation, 2);
        assert_eq!(s.inventory.bot_loot_generation, 0);
        reduce(
            &mut s,
            ProtocolObservation::LootOpened {
                target: id,
                ownership: crate::LootOwnership::Bot,
            },
        );
        reduce(
            &mut s,
            ProtocolObservation::LootClosed {
                ownership: crate::LootOwnership::Bot,
            },
        );
        assert_eq!(s.inventory.bot_loot_generation, 2);
    }

    #[test]
    fn talent_tree_resolves_when_class_and_active_talents_are_authoritative() {
        let mut state = AuthoritativeState::default();
        reduce(
            &mut state,
            ProtocolObservation::PlayerTalents {
                group_count: Some(2),
                active_group: Some(1),
                talents: vec![
                    crate::capabilities::TalentRank {
                        talent_id: 74,
                        rank: 1,
                    },
                    crate::capabilities::TalentRank {
                        talent_id: 27,
                        rank: 3,
                    },
                ],
                glyph_properties: Some(vec![871]),
            },
        );
        assert_eq!(state.capabilities.specialization_tree, None);
        reduce(&mut state, ProtocolObservation::PlayerClass { class_id: 8 });
        assert_eq!(state.capabilities.active_talent_group, Some(1));
        assert_eq!(state.capabilities.active_glyph_properties, Some(vec![871]));
        assert_eq!(state.capabilities.specialization_tree, Some(1));

        reduce(
            &mut state,
            ProtocolObservation::PlayerTalents {
                group_count: None,
                active_group: None,
                talents: Vec::new(),
                glyph_properties: None,
            },
        );
        assert_eq!(state.capabilities.specialization_tree, None);
        assert_eq!(state.capabilities.active_glyph_properties, None);
    }
}
