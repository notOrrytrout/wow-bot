use wow_domain::EntityId;
use wow_state::{Snapshot, inventory::InventoryItemInstance};

const POTION_SICKNESS_AURA: u32 = 53_787;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PotionKinds {
    pub health: bool,
    pub mana: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CombatPotionUse {
    pub item: u32,
    pub instance: InventoryItemInstance,
    pub spell: u32,
}

/// Classify a potion from authoritative item-template metadata and reviewed name cues.
pub fn potion_kinds(metadata: &wow_state::inventory::ItemTemplateMetadata) -> PotionKinds {
    if metadata.item_class != 0 || metadata.subclass != 1 || metadata.use_spell_id == 0 {
        return PotionKinds::default();
    }
    let name = metadata.name.to_ascii_lowercase();
    PotionKinds {
        health: [
            "healing potion",
            "health potion",
            "healing injector",
            "health injector",
            "regeneration potion",
            "rejuvenation potion",
        ]
        .iter()
        .any(|cue| name.contains(cue)),
        mana: ["mana potion", "mana injector", "rejuvenation potion"]
            .iter()
            .any(|cue| name.contains(cue)),
    }
}

/// Select a critical combat potion only when all required player and item facts are authoritative.
pub fn select_combat_potion(
    snapshot: &Snapshot,
    target: EntityId,
    health_threshold: Option<(u64, bool)>,
    mana_threshold: Option<(u64, bool)>,
    shared_lockout_ready: bool,
) -> Option<CombatPotionUse> {
    let player = EntityId(snapshot.state.session.character_guid?);
    if !snapshot.state.session.in_world
        || !snapshot.state.auras.by_entity.contains_key(&player)
        || !shared_lockout_ready
        || has_potion_sickness(snapshot, player)
    {
        return None;
    }

    let health_percent = snapshot
        .state
        .entities
        .0
        .get(&player)
        .and_then(|entity| entity.health)
        .filter(|(_, max)| *max > 0)
        .map(|(health, max)| health.saturating_mul(100) / max);
    let health_critical = health_threshold.is_some_and(|(threshold, self_heal_available)| {
        health_percent.is_some_and(|health| {
            u64::from(health) <= 15 || (u64::from(health) <= threshold && !self_heal_available)
        })
    });

    let mana = snapshot.state.entities.0.get(&player).and_then(|entity| {
        (entity.power_type == Some(0))
            .then_some(entity.power)
            .flatten()
    });
    let mana_critical = mana_threshold.is_some_and(|(threshold, ongoing_work)| {
        ongoing_work
            && mana.is_some_and(|(current, maximum)| {
                maximum > 0
                    && u64::from(current).saturating_mul(100)
                        <= u64::from(maximum).saturating_mul(threshold)
            })
    });
    if !health_critical && !mana_critical {
        return None;
    }

    snapshot
        .state
        .inventory
        .instances
        .values()
        .filter(|instance| instance.count > 0 && instance.backpack_slot >= 23)
        .filter_map(|instance| {
            let metadata = snapshot.state.inventory.item_metadata.get(&instance.item)?;
            let kinds = potion_kinds(metadata);
            let matches_need = if health_critical {
                kinds.health
            } else {
                mana_critical && kinds.mana
            };
            (matches_need && metadata.use_spell_id != 0).then_some(CombatPotionUse {
                item: instance.item,
                instance: instance.clone(),
                spell: metadata.use_spell_id,
            })
        })
        .min_by_key(|potion| (potion.instance.backpack_slot, potion.instance.guid))
        .filter(|_| snapshot.state.entities.0.contains_key(&target))
}

fn has_potion_sickness(snapshot: &Snapshot, player: EntityId) -> bool {
    snapshot
        .state
        .auras
        .by_entity
        .get(&player)
        .into_iter()
        .flat_map(|auras| auras.values())
        .any(|aura| aura.spell == POTION_SICKNESS_AURA)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, entities::EntityState, inventory::ItemTemplateMetadata};

    fn potion_state() -> (AuthoritativeState, EntityId) {
        let player = EntityId(1);
        let target = EntityId(2);
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(player.0);
        state.auras.by_entity.insert(player, Default::default());
        state.entities.0.insert(
            player,
            EntityState {
                health: Some((15, 100)),
                power: Some((10, 100)),
                power_type: Some(0),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            target,
            EntityState {
                health: Some((80, 100)),
                ..Default::default()
            },
        );
        state.inventory.instances.insert(
            EntityId(11),
            InventoryItemInstance {
                item: 101,
                guid: EntityId(11),
                backpack_slot: 23,
                count: 1,
            },
        );
        state.inventory.item_metadata.insert(
            101,
            ItemTemplateMetadata {
                name: "Healing Potion".into(),
                item_class: 0,
                subclass: 1,
                use_spell_id: 900,
                ..Default::default()
            },
        );
        (state, target)
    }

    #[test]
    fn critical_health_selects_authoritative_backpack_instance() {
        let (mut state, target) = potion_state();
        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((20, 100));
        assert!(
            select_combat_potion(
                &Snapshot::from_state(&state),
                target,
                Some((25, true)),
                None,
                true
            )
            .is_none()
        );
        let selected = select_combat_potion(
            &Snapshot::from_state(&state),
            target,
            Some((25, false)),
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            (selected.item, selected.instance.guid, selected.spell),
            (101, EntityId(11), 900)
        );
    }

    #[test]
    fn fifteen_percent_health_uses_potion_even_when_a_self_heal_is_ready() {
        let (state, target) = potion_state();
        assert!(
            select_combat_potion(
                &Snapshot::from_state(&state),
                target,
                Some((25, true)),
                None,
                true
            )
            .is_some()
        );
    }

    #[test]
    fn potion_selection_respects_sickness_unknown_aura_and_shared_lockout() {
        let (mut state, target) = potion_state();
        state.auras.by_entity.remove(&EntityId(1));
        assert!(
            select_combat_potion(
                &Snapshot::from_state(&state),
                target,
                Some((25, false)),
                None,
                true
            )
            .is_none()
        );
        state
            .auras
            .by_entity
            .insert(EntityId(1), Default::default());
        assert!(
            select_combat_potion(
                &Snapshot::from_state(&state),
                target,
                Some((25, false)),
                None,
                false
            )
            .is_none()
        );
        state.auras.by_entity.get_mut(&EntityId(1)).unwrap().insert(
            0,
            wow_state::auras::AuraInstance {
                slot: 0,
                spell: POTION_SICKNESS_AURA,
                positive: Some(true),
                caster: Some(EntityId(1)),
                max_duration_ms: None,
                remaining_ms: None,
                observed_at_ms: None,
            },
        );
        assert!(
            select_combat_potion(
                &Snapshot::from_state(&state),
                target,
                Some((25, false)),
                None,
                true
            )
            .is_none()
        );
    }

    #[test]
    fn mana_potion_requires_current_work_and_low_mana() {
        let (mut state, target) = potion_state();
        state.inventory.item_metadata.get_mut(&101).unwrap().name = "Mana Potion".into();
        assert!(
            select_combat_potion(
                &Snapshot::from_state(&state),
                target,
                None,
                Some((15, false)),
                true
            )
            .is_none()
        );
        assert!(
            select_combat_potion(
                &Snapshot::from_state(&state),
                target,
                None,
                Some((15, true)),
                true
            )
            .is_some()
        );
    }
}
