use std::collections::{BTreeMap, BTreeSet};
use wow_domain::EntityId;
use wow_state::Snapshot;

pub const BANK_BAG_PRESSURE_FREE_SLOTS: u16 = 2;
pub const BANK_INTERACTION_RANGE_YARDS: f32 = 5.0;
pub const BACKPACK_SLOT_RANGE: std::ops::RangeInclusive<u8> = 23..=38;
const NPC_FLAG_BANKER: u32 = 0x0000_0008;

pub fn is_banker_flags(flags: u32) -> bool {
    flags & NPC_FLAG_BANKER != 0
}

pub fn protected_item_id_set(item_ids: &[u32]) -> BTreeSet<u32> {
    item_ids.iter().copied().collect()
}

pub fn bank_work_allowed(snapshot: &Snapshot) -> bool {
    let Some(player) = snapshot.state.session.character_guid.map(EntityId) else {
        return false;
    };
    let Some(entity) = snapshot.state.entities.0.get(&player) else {
        return false;
    };
    snapshot.state.session.in_world
        && crate::safety::player_can_do_stationary_work(snapshot)
        && !snapshot.state.active_casts.contains_key(&player)
        && entity.health.is_some_and(|(current, _)| current > 0)
        && entity.in_combat() != Some(true)
        && !snapshot
            .state
            .auras
            .has_any(player, &[wow_state::life::GHOST_AURA_SPELL_ID])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BankDepositCandidate {
    pub banker: EntityId,
    pub item: u32,
    pub item_guid: EntityId,
    pub backpack_slot: u8,
}

pub fn bag_pressure_requires_bank(snapshot: &Snapshot) -> bool {
    bank_work_allowed(snapshot)
        && snapshot.state.inventory.instances_authoritative
        && snapshot.state.inventory.free_slots <= BANK_BAG_PRESSURE_FREE_SLOTS
}

pub fn trusted_nearby_bankers(snapshot: &Snapshot) -> Vec<EntityId> {
    let Some(player) = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)
    else {
        return Vec::new();
    };
    let mut bankers = snapshot
        .state
        .entities
        .0
        .values()
        .filter(|entity| {
            crate::interaction::is_nearby_interactable_unit(
                entity,
                player,
                BANK_INTERACTION_RANGE_YARDS,
            ) && entity.npc_flags.is_some_and(is_banker_flags)
        })
        .map(|entity| entity.id)
        .collect::<Vec<_>>();
    bankers.sort_unstable();
    bankers
}

pub fn remembered_banker_destination(
    snapshot: &Snapshot,
    remembered: &BTreeMap<u32, (wow_domain::WorldPosition, std::time::Instant)>,
    now: std::time::Instant,
    max_age: std::time::Duration,
    max_distance: f32,
) -> Option<wow_domain::WorldPosition> {
    let position = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)?;
    remembered
        .get(&position.map)
        .filter(|(_, observed_at)| now.saturating_duration_since(*observed_at) <= max_age)
        .map(|(banker, _)| *banker)
        .filter(|banker| banker.point.distance(position.point) <= max_distance)
}

/// Choose backpack trade goods for one validated deposit action.
/// Other item classes and incomplete instance/template observations fail closed.
pub fn profession_material_deposit_candidates(
    snapshot: &Snapshot,
    banker: EntityId,
    protected_item_ids: &BTreeSet<u32>,
) -> Vec<BankDepositCandidate> {
    if !snapshot.state.inventory.instances_authoritative {
        return Vec::new();
    }
    let quest_items = snapshot
        .state
        .quests
        .active
        .keys()
        .filter_map(|quest| snapshot.state.quests.definitions.get(quest))
        .flat_map(|quest| quest.items.iter().map(|objective| objective.item))
        .collect::<BTreeSet<_>>();
    let spell_reagents = crate::combat::spells::reagent_item_counts(
        snapshot.state.capabilities.spells.iter().copied(),
    )
    .into_keys()
    .collect::<BTreeSet<_>>();
    let mut candidates = snapshot
        .state
        .inventory
        .instances
        .values()
        .filter_map(|instance| {
            if instance.count == 0
                || !BACKPACK_SLOT_RANGE.contains(&instance.backpack_slot)
                || protected_item_ids.contains(&instance.item)
                || quest_items.contains(&instance.item)
                || spell_reagents.contains(&instance.item)
            {
                return None;
            }
            let metadata = snapshot.state.inventory.item_metadata.get(&instance.item)?;
            if metadata.item_class != 7 {
                return None;
            }
            Some(BankDepositCandidate {
                banker,
                item: instance.item,
                item_guid: instance.guid,
                backpack_slot: instance.backpack_slot,
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| (candidate.backpack_slot, candidate.item_guid));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{EntityId, WorldPosition};
    use wow_state::{
        AuthoritativeState, Snapshot,
        entities::{EntityKind, EntityState},
        inventory::{InventoryItemInstance, ItemTemplateMetadata},
        quests::{QuestDefinition, QuestItemObjective, QuestProgress},
    };

    fn snapshot_with_banker() -> (Snapshot, EntityId) {
        let banker = EntityId(40);
        let mut state = AuthoritativeState::default();
        let player = EntityId(1);
        state.session.in_world = true;
        state.session.character_guid = Some(player.0);
        state.position.player = Some(WorldPosition {
            map: 0,
            point: wow_domain::Vec3::default(),
            orientation: 0.0,
        });
        state.entities.0.insert(
            player,
            EntityState {
                id: player,
                kind: EntityKind::Player,
                health: Some((100, 100)),
                position: state.position.player,
                ..Default::default()
            },
        );
        state.entities.0.insert(
            banker,
            EntityState {
                id: banker,
                kind: EntityKind::Unit,
                interactable: true,
                npc_flags: Some(NPC_FLAG_BANKER),
                position: Some(WorldPosition {
                    map: 0,
                    point: wow_domain::Vec3 {
                        x: 1.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        state.inventory.instances_authoritative = true;
        state.inventory.bank = wow_state::inventory::BankState {
            authoritative: true,
            banker: Some(banker),
        };
        state.inventory.instances.insert(
            EntityId(100),
            InventoryItemInstance {
                item: 2589,
                guid: EntityId(100),
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
        (Snapshot::from_state(&state), banker)
    }

    #[test]
    fn nearby_banker_requires_current_unit_service_flag_and_range() {
        let (snapshot, banker) = snapshot_with_banker();
        assert_eq!(trusted_nearby_bankers(&snapshot), vec![banker]);
        let mut state = snapshot.state.clone();
        state.entities.0.get_mut(&banker).unwrap().npc_flags = Some(0);
        assert!(trusted_nearby_bankers(&Snapshot::from_state(&state)).is_empty());
    }

    #[test]
    fn bank_work_stops_for_combat_death_or_movement() {
        let (snapshot, _) = snapshot_with_banker();
        assert!(bank_work_allowed(&snapshot));

        let mut state = snapshot.state.clone();
        state.position.moving = true;
        assert!(!bank_work_allowed(&Snapshot::from_state(&state)));
        state.position.moving = false;
        state.control.mover = Some(EntityId(1));
        assert!(!bank_work_allowed(&Snapshot::from_state(&state)));
        state.control.mover = None;
        state.entities.0.get_mut(&EntityId(1)).unwrap().unit_flags = Some(0x0008_0000);
        assert!(!bank_work_allowed(&Snapshot::from_state(&state)));
        state.entities.0.get_mut(&EntityId(1)).unwrap().unit_flags = Some(0);
        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((0, 100));
        assert!(!bank_work_allowed(&Snapshot::from_state(&state)));
    }

    #[test]
    fn deposits_only_known_profession_materials_and_honors_protected_sets() {
        let (snapshot, banker) = snapshot_with_banker();
        assert_eq!(
            profession_material_deposit_candidates(&snapshot, banker, &BTreeSet::new()),
            vec![BankDepositCandidate {
                banker,
                item: 2589,
                item_guid: EntityId(100),
                backpack_slot: 23,
            }]
        );
        assert!(
            profession_material_deposit_candidates(&snapshot, banker, &BTreeSet::from([2589]))
                .is_empty()
        );
    }

    #[test]
    fn quest_objectives_and_non_trade_good_classes_are_never_deposited() {
        let (mut snapshot, banker) = snapshot_with_banker();
        snapshot
            .state
            .quests
            .active
            .insert(12, QuestProgress::default());
        snapshot.state.quests.definitions.insert(
            12,
            QuestDefinition {
                quest: 12,
                title: "Active objective".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: Vec::new(),
                items: vec![QuestItemObjective {
                    item: 2589,
                    required: 2,
                }],
            },
        );
        assert!(
            profession_material_deposit_candidates(&snapshot, banker, &BTreeSet::new()).is_empty()
        );
        snapshot.state.quests.active.clear();
        snapshot
            .state
            .inventory
            .item_metadata
            .get_mut(&2589)
            .unwrap()
            .item_class = 5;
        assert!(
            profession_material_deposit_candidates(&snapshot, banker, &BTreeSet::new()).is_empty()
        );
    }

    #[test]
    fn reagents_for_known_spells_are_never_deposited() {
        let (mut snapshot, banker) = snapshot_with_banker();
        snapshot.state.capabilities.spells.insert(130); // Slow Fall requires Light Feather.
        snapshot.state.inventory.instances.insert(
            EntityId(101),
            InventoryItemInstance {
                item: 17_056,
                guid: EntityId(101),
                backpack_slot: 24,
                count: 5,
            },
        );
        snapshot.state.inventory.item_metadata.insert(
            17_056,
            ItemTemplateMetadata {
                item_class: 7,
                ..Default::default()
            },
        );
        let candidates =
            profession_material_deposit_candidates(&snapshot, banker, &BTreeSet::new());
        assert!(candidates.iter().all(|candidate| candidate.item != 17_056));
    }

    #[test]
    fn remembered_banker_location_requires_current_map_recent_observation_and_short_range() {
        let now = std::time::Instant::now();
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition {
            map: 4,
            point: wow_domain::Vec3::new(10.0, 10.0, 0.0),
            orientation: 0.0,
        });
        let snapshot = Snapshot::from_state(&state);
        let position = WorldPosition {
            map: 4,
            point: wow_domain::Vec3::new(39.0, 10.0, 0.0),
            orientation: 0.0,
        };
        let memory = BTreeMap::from([(4, (position, now))]);
        assert_eq!(
            remembered_banker_destination(
                &snapshot,
                &memory,
                now,
                std::time::Duration::from_secs(30 * 60),
                30.0,
            ),
            Some(position)
        );
        assert_eq!(
            remembered_banker_destination(
                &snapshot,
                &BTreeMap::from([(3, (position, now))]),
                now,
                std::time::Duration::from_secs(30 * 60),
                30.0,
            ),
            None
        );
        assert_eq!(
            remembered_banker_destination(
                &snapshot,
                &memory,
                now + std::time::Duration::from_secs(30 * 60 + 1),
                std::time::Duration::from_secs(30 * 60),
                30.0,
            ),
            None
        );
        let far = WorldPosition {
            point: wow_domain::Vec3::new(41.0, 10.0, 0.0),
            ..position
        };
        assert_eq!(
            remembered_banker_destination(
                &snapshot,
                &BTreeMap::from([(4, (far, now))]),
                now,
                std::time::Duration::from_secs(30 * 60),
                30.0,
            ),
            None
        );
    }
}
