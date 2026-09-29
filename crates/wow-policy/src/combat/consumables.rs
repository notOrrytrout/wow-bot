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

pub type CombatItemUse = CombatPotionUse;

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

    select_backpack_use_item(snapshot, |metadata| {
        let kinds = potion_kinds(metadata);
        if health_critical {
            kinds.health
        } else {
            mana_critical && kinds.mana
        }
    })
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

/// Select an emergency healthstone from a current backpack instance at the legacy 30% gate.
pub fn select_emergency_healthstone(
    snapshot: &Snapshot,
    target: EntityId,
) -> Option<CombatItemUse> {
    let player = emergency_player(snapshot, target)?;
    let health = player_health_percent(snapshot, player)?;
    if health > 30 {
        return None;
    }
    select_backpack_use_item(snapshot, |metadata| {
        item_name_has_cue(metadata, &["healthstone"])
    })
}

/// Select the old non-potion emergency items at the health-potion survival gate.
/// These items do not use Potion Sickness or the shared potion lockout.
pub fn select_emergency_health_item(
    snapshot: &Snapshot,
    target: EntityId,
    self_heal_available: bool,
) -> Option<CombatItemUse> {
    let player = emergency_player(snapshot, target)?;
    let health = player_health_percent(snapshot, player)?;
    if health > 25 || (health > 15 && self_heal_available) {
        return None;
    }
    select_backpack_use_item(snapshot, |metadata| {
        item_name_has_cue(metadata, &["whipper root tuber"])
    })
    .or_else(|| {
        select_backpack_use_item(snapshot, |metadata| {
            item_name_has_cue(metadata, &["night dragon's breath"])
        })
    })
}

fn emergency_player(snapshot: &Snapshot, target: EntityId) -> Option<EntityId> {
    let player = EntityId(snapshot.state.session.character_guid?);
    (snapshot.state.session.in_world
        && snapshot.state.entities.0.contains_key(&player)
        && snapshot.state.entities.0.contains_key(&target)
        && !spirit_of_redemption_active(snapshot, player))
    .then_some(player)
}

fn spirit_of_redemption_active(snapshot: &Snapshot, player: EntityId) -> bool {
    snapshot.state.capabilities.class_id == Some(5)
        && snapshot
            .state
            .auras
            .by_entity
            .get(&player)
            .is_some_and(|auras| auras.values().any(|aura| aura.spell == 27_827))
}

fn player_health_percent(snapshot: &Snapshot, player: EntityId) -> Option<u32> {
    snapshot
        .state
        .entities
        .0
        .get(&player)
        .and_then(|entity| entity.health)
        .filter(|(_, maximum)| *maximum > 0)
        .map(|(health, maximum)| health.saturating_mul(100) / maximum)
}

fn item_name_has_cue(metadata: &wow_state::inventory::ItemTemplateMetadata, cues: &[&str]) -> bool {
    metadata.item_class == 0 && metadata.use_spell_id != 0 && {
        let name = metadata.name.to_ascii_lowercase();
        cues.iter().any(|cue| name.contains(cue))
    }
}

fn select_backpack_use_item(
    snapshot: &Snapshot,
    matches: impl Fn(&wow_state::inventory::ItemTemplateMetadata) -> bool,
) -> Option<CombatItemUse> {
    snapshot
        .state
        .inventory
        .instances
        .values()
        .filter(|instance| instance.count > 0 && instance.backpack_slot >= 23)
        .filter_map(|instance| {
            let metadata = snapshot.state.inventory.item_metadata.get(&instance.item)?;
            matches(metadata).then_some(CombatItemUse {
                item: instance.item,
                instance: instance.clone(),
                spell: metadata.use_spell_id,
            })
        })
        .min_by_key(|item| (item.instance.backpack_slot, item.instance.guid))
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

    fn add_backpack_use_item(state: &mut AuthoritativeState, guid: u64, item: u32, name: &str) {
        state.inventory.instances.insert(
            EntityId(guid),
            InventoryItemInstance {
                item,
                guid: EntityId(guid),
                backpack_slot: 24,
                count: 1,
            },
        );
        state.inventory.item_metadata.insert(
            item,
            ItemTemplateMetadata {
                name: name.into(),
                item_class: 0,
                use_spell_id: 901,
                ..Default::default()
            },
        );
    }

    #[test]
    fn healthstone_uses_the_legacy_thirty_percent_gate_and_current_backpack_template() {
        let (mut state, target) = potion_state();
        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((30, 100));
        add_backpack_use_item(&mut state, 12, 102, "Minor Healthstone");
        let selected = select_emergency_healthstone(&Snapshot::from_state(&state), target).unwrap();
        assert_eq!((selected.item, selected.instance.guid), (102, EntityId(12)));

        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((31, 100));
        assert!(select_emergency_healthstone(&Snapshot::from_state(&state), target).is_none());
        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((20, 100));
        state.inventory.item_metadata.remove(&102);
        assert!(select_emergency_healthstone(&Snapshot::from_state(&state), target).is_none());
    }

    #[test]
    fn spirit_of_redemption_suppresses_emergency_health_items() {
        let (mut state, target) = potion_state();
        state.capabilities.class_id = Some(5);
        add_backpack_use_item(&mut state, 12, 102, "Minor Healthstone");
        state.auras.by_entity.get_mut(&EntityId(1)).unwrap().insert(
            0,
            wow_state::auras::AuraInstance {
                slot: 0,
                spell: 27_827,
                positive: Some(true),
                caster: Some(EntityId(1)),
                max_duration_ms: None,
                remaining_ms: None,
                observed_at_ms: None,
            },
        );
        let snapshot = Snapshot::from_state(&state);
        assert!(select_emergency_healthstone(&snapshot, target).is_none());
        assert!(select_emergency_health_item(&snapshot, target, false).is_none());
    }

    #[test]
    fn non_potion_emergency_items_use_critical_health_gate_without_potion_sickness() {
        let (mut state, target) = potion_state();
        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((20, 100));
        add_backpack_use_item(&mut state, 12, 102, "Whipper Root Tuber");
        add_backpack_use_item(&mut state, 13, 103, "Night Dragon's Breath");
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
        let selected =
            select_emergency_health_item(&Snapshot::from_state(&state), target, false).unwrap();
        assert_eq!((selected.item, selected.instance.guid), (102, EntityId(12)));

        assert!(
            select_emergency_health_item(&Snapshot::from_state(&state), target, true).is_none()
        );
        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((15, 100));
        let selected =
            select_emergency_health_item(&Snapshot::from_state(&state), target, true).unwrap();
        assert_eq!((selected.item, selected.instance.guid), (102, EntityId(12)));

        state.inventory.instances.remove(&EntityId(12));
        let selected =
            select_emergency_health_item(&Snapshot::from_state(&state), target, true).unwrap();
        assert_eq!((selected.item, selected.instance.guid), (103, EntityId(13)));
    }

    #[test]
    fn non_potion_emergency_items_require_authoritative_use_metadata_and_backpack_slot() {
        let (mut state, target) = potion_state();
        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((15, 100));
        add_backpack_use_item(&mut state, 12, 102, "Whipper Root Tuber");
        state
            .inventory
            .instances
            .get_mut(&EntityId(12))
            .unwrap()
            .backpack_slot = 10;
        assert!(
            select_emergency_health_item(&Snapshot::from_state(&state), target, true).is_none()
        );
        state
            .inventory
            .instances
            .get_mut(&EntityId(12))
            .unwrap()
            .backpack_slot = 24;
        state
            .inventory
            .item_metadata
            .get_mut(&102)
            .unwrap()
            .use_spell_id = 0;
        assert!(
            select_emergency_health_item(&Snapshot::from_state(&state), target, true).is_none()
        );
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
