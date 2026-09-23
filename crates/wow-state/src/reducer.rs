use crate::{AuthoritativeState, ProtocolObservation, StateDelta};

pub fn reduce(state: &mut AuthoritativeState, observation: ProtocolObservation) -> StateDelta {
    let mut delta = StateDelta::default();
    match observation {
        ProtocolObservation::Authenticated { realm } => { state.session.authenticated = true; state.session.realm = realm; delta.changed.push("session".into()); }
        ProtocolObservation::EnteredWorld { character_guid, position } => { state.session.in_world = true; state.session.character_guid = Some(character_guid); state.position.player = position; delta.changed.extend(["session".into(), "position".into()]); }
        ProtocolObservation::LeftWorld => { state.session.in_world = false; state.position.moving = false; state.control = Default::default(); state.auras = Default::default(); delta.changed.extend(["session".into(), "control".into(), "auras".into()]); }
        ProtocolObservation::PlayerPosition { position, moving, flags, client_time } => { state.position.player = Some(position); state.position.moving = moving; state.position.flags = flags; state.position.client_time = client_time; delta.changed.push("position".into()); }
        ProtocolObservation::ControlledMover { mover, position, flags } => { state.control.mover = mover; state.control.mover_position = position; state.control.movement_flags = flags; if mover.is_none() { state.control.abilities.clear(); } delta.changed.push("control".into()); }
        ProtocolObservation::ControlledAbilities { mover, spells } => { if state.control.mover == Some(mover) { state.control.abilities = spells.into_iter().collect(); delta.changed.push("control".into()); } }
        ProtocolObservation::CastFailed { .. } => {}
        ProtocolObservation::CorpseLocation { position } => { state.life.corpse=position; state.life.recovery_generation=state.life.recovery_generation.wrapping_add(1); delta.changed.push("life".into()); }
        ProtocolObservation::CorpseReclaimDelay { ready_at_ms } => { state.life.reclaim_ready_at_ms=ready_at_ms; delta.changed.push("life".into()); }
        ProtocolObservation::EntityUpsert { entity } => { delta.touched_entities.push(entity.id); state.entities.0.insert(entity.id, entity); delta.changed.push("entities".into()); }
        ProtocolObservation::EntityRemoved { entity } => {
            state.entities.0.remove(&entity);
            if state.inventory.vendor == Some(entity) { state.inventory.vendor = None; }
            if state.inventory.current_loot == Some(entity) { state.inventory.current_loot = None; state.inventory.current_loot_owner = None; state.inventory.loot_generation = state.inventory.loot_generation.wrapping_add(1); }
            if state.inventory.trade.partner == Some(entity) { state.inventory.trade.open = false; state.inventory.trade.generation = state.inventory.trade.generation.wrapping_add(1); }
            delta.touched_entities.push(entity); delta.changed.extend(["entities".into(), "inventory".into()]);
        }
        ProtocolObservation::InventoryCount { item, count } => { if count == 0 { state.inventory.items.remove(&item); } else { state.inventory.items.insert(item, count); } delta.changed.push("inventory".into()); }
        ProtocolObservation::InventoryInstances { items } => { state.inventory.instances = items.into_iter().map(|instance| (instance.item, instance)).collect(); delta.changed.push("inventory".into()); }
        ProtocolObservation::InventoryFreeSlots { count } => { state.inventory.free_slots = count; delta.changed.push("inventory".into()); }
        ProtocolObservation::EquippedRangedItem { item } => { state.inventory.equipped_ranged_item = item; state.inventory.equipment_authoritative = true; delta.changed.push("inventory".into()); }
        ProtocolObservation::Money { copper } => { state.inventory.money = copper; delta.changed.push("inventory".into()); }
        ProtocolObservation::LootOpened { target, ownership } => {
            state.inventory.current_loot = Some(target);
            state.inventory.current_loot_owner = Some(ownership);
            state.inventory.loot_generation = state.inventory.loot_generation.wrapping_add(1);
            if matches!(ownership, crate::observation::LootOwnership::Bot) { state.inventory.bot_loot_generation = state.inventory.bot_loot_generation.wrapping_add(1); }
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::LootClosed { ownership } => {
            state.inventory.current_loot = None;
            state.inventory.current_loot_owner = None;
            state.inventory.loot_generation = state.inventory.loot_generation.wrapping_add(1);
            if matches!(ownership, crate::observation::LootOwnership::Bot) { state.inventory.bot_loot_generation = state.inventory.bot_loot_generation.wrapping_add(1); }
            delta.changed.push("inventory".into());
        }
        ProtocolObservation::VendorOpened { vendor } => { state.inventory.vendor = Some(vendor); delta.changed.push("inventory".into()); }
        ProtocolObservation::VendorClosed => { state.inventory.vendor = None; delta.changed.push("inventory".into()); }
        ProtocolObservation::Trade(trade) => { state.inventory.trade = trade; delta.changed.push("inventory".into()); }
        ProtocolObservation::Auction(auction) => { state.inventory.auction = auction; delta.changed.push("inventory".into()); }
        ProtocolObservation::Mailbox(mailbox) => { state.inventory.mailbox = mailbox; delta.changed.push("inventory".into()); }
        ProtocolObservation::QuestGiverStatus { giver, status } => {
            state.quests.giver_status.insert(giver, status);
            if !matches!(status, 2 | 4 | 7 | 8) {
                state.quests.offers.retain(|_, offer| offer.giver != giver);
            }
            state.entities.0.entry(giver).or_insert_with(|| crate::entities::EntityState { id: giver, interactable: true, ..Default::default() });
            delta.touched_entities.push(giver);
            delta.changed.extend(["quests".into(), "entities".into()]);
        }
        ProtocolObservation::QuestOffer { giver, quest, icon } => {
            state.quests.offers.insert(quest, crate::quests::QuestOffer { giver, icon });
            state.entities.0.entry(giver).or_insert_with(|| crate::entities::EntityState { id: giver, interactable: true, ..Default::default() });
            delta.touched_entities.push(giver);
            delta.changed.extend(["quests".into(), "entities".into()]);
        }
        ProtocolObservation::QuestAccepted { quest } => { state.quests.active.entry(quest).or_default(); state.quests.offers.remove(&quest); delta.changed.push("quests".into()); }
        ProtocolObservation::QuestDefinition { definition } => { state.quests.definitions.insert(definition.quest, definition); delta.changed.push("quests".into()); }
        ProtocolObservation::QuestTurnInDialog { quest, dialog } => { state.quests.turn_in.insert(quest, dialog); delta.changed.push("quests".into()); }
        ProtocolObservation::QuestProgress { quest, objectives, complete } => { state.quests.active.insert(quest, crate::quests::QuestProgress { complete, objectives }); delta.changed.push("quests".into()); }
        ProtocolObservation::QuestRemoved { quest } => {
            let was_turning_in = state.quests.turn_in.contains_key(&quest);
            let was_complete = state.quests.active.get(&quest).is_some_and(|progress| progress.complete);
            state.quests.active.remove(&quest);
            state.quests.turn_in.remove(&quest);
            if was_turning_in && was_complete && !state.quests.completed.contains(&quest) { state.quests.completed.push(quest); }
            delta.changed.push("quests".into());
        }
        ProtocolObservation::QuestCompleted { quest } => { state.quests.active.remove(&quest); if !state.quests.completed.contains(&quest) { state.quests.completed.push(quest); } delta.changed.push("quests".into()); }
        ProtocolObservation::SpellKnown { spell } => { state.capabilities.spells.insert(spell); delta.changed.push("capabilities".into()); }
        ProtocolObservation::SpellCooldown { spell, ready_at_ms } => { state.capabilities.spell_cooldowns.insert(spell, ready_at_ms); delta.changed.push("capabilities".into()); }
        ProtocolObservation::PlayerClass { class_id } => { state.capabilities.class_id = Some(class_id); delta.changed.push("capabilities".into()); }
        ProtocolObservation::AuraSnapshot { entity, auras } => { state.auras.by_entity.insert(entity, auras.into_iter().map(|aura| (aura.slot, aura)).collect()); delta.changed.push("auras".into()); }
        ProtocolObservation::AuraSlot { entity, slot, aura } => { let slots = state.auras.by_entity.entry(entity).or_default(); if let Some(aura) = aura { slots.insert(slot, aura); } else { slots.remove(&slot); } delta.changed.push("auras".into()); }
        ProtocolObservation::Skill { skill, current, max } => { state.professions.skills.insert(skill, (current, max)); delta.changed.push("professions".into()); }
        ProtocolObservation::RecipeKnown { recipe } => { state.professions.known_recipes.insert(recipe); delta.changed.push("professions".into()); }
        ProtocolObservation::ProfessionFlags { cooking, first_aid, fishing } => { state.professions.cooking = cooking; state.professions.first_aid = first_aid; state.professions.fishing = fishing; state.capabilities.can_fish = fishing; delta.changed.extend(["professions".into(), "capabilities".into()]); }
        ProtocolObservation::Group(group) => { state.group = group; delta.changed.push("group".into()); }
        ProtocolObservation::Desync { reason } => { state.desync.suspect = true; state.desync.reasons.push(reason); delta.changed.push("desync".into()); }
        ProtocolObservation::Resynchronized => { state.desync = Default::default(); delta.changed.push("desync".into()); }
        ProtocolObservation::Raw { .. } => {}
    }
    state.revision = state.revision.next(); delta.revision = state.revision; delta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::EntityState;
    use wow_domain::EntityId;
    #[test]
    fn removing_entity_clears_dependent_service_state(){
        let mut s=AuthoritativeState::default();let id=EntityId(7);s.entities.0.insert(id,EntityState{id,..Default::default()});s.inventory.vendor=Some(id);s.inventory.current_loot=Some(id);s.inventory.trade.partner=Some(id);s.inventory.trade.open=true;
        reduce(&mut s,ProtocolObservation::EntityRemoved{entity:id});
        assert!(!s.entities.0.contains_key(&id));assert_eq!(s.inventory.vendor,None);assert_eq!(s.inventory.current_loot,None);assert!(!s.inventory.trade.open);
    }
    #[test]
    fn player_loot_does_not_advance_bot_loot_generation(){
        let mut s=AuthoritativeState::default();
        let id=EntityId(9);
        reduce(&mut s,ProtocolObservation::LootOpened{target:id,ownership:crate::LootOwnership::Player});
        assert_eq!(s.inventory.loot_generation,1);
        assert_eq!(s.inventory.bot_loot_generation,0);
        reduce(&mut s,ProtocolObservation::LootClosed{ownership:crate::LootOwnership::Player});
        assert_eq!(s.inventory.loot_generation,2);
        assert_eq!(s.inventory.bot_loot_generation,0);
        reduce(&mut s,ProtocolObservation::LootOpened{target:id,ownership:crate::LootOwnership::Bot});
        reduce(&mut s,ProtocolObservation::LootClosed{ownership:crate::LootOwnership::Bot});
        assert_eq!(s.inventory.bot_loot_generation,2);
    }

}
