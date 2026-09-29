use wow_domain::EntityId;
use wow_state::{Snapshot, inventory::ItemTemplateMetadata};

pub fn score_improves_by_fraction(
    new_score: f32,
    current_score: f32,
    minimum_fraction: f32,
) -> bool {
    let fraction = minimum_fraction.max(0.0);
    let minimum_delta = current_score.abs().max(1.0) * fraction;
    new_score > current_score + minimum_delta
}

pub fn destination_slots(metadata: &ItemTemplateMetadata) -> &'static [u8] {
    match metadata.inventory_type {
        1 => &[0],                  // head
        2 => &[1],                  // neck
        3 => &[2],                  // shoulders
        5 | 20 => &[4],             // chest / robe
        6 => &[5],                  // waist
        7 => &[6],                  // legs
        8 => &[7],                  // feet
        9 => &[8],                  // wrists
        10 => &[9],                 // hands
        11 => &[10, 11],            // fingers
        12 => &[12, 13],            // trinkets
        16 => &[14],                // cloak
        13 | 17 | 21 => &[15],      // weapon / 2h / main hand
        14 | 22 | 23 => &[16],      // shield / off hand / holdable
        15 | 25 | 26 | 28 => &[17], // ranged / thrown / ranged-right / relic
        _ => &[],
    }
}

pub fn player_can_use(metadata: &ItemTemplateMetadata, class: u8, level: u8) -> bool {
    if metadata.required_level > u32::from(level) {
        return false;
    }
    if metadata.allowable_class != 0 && metadata.allowable_class != u32::MAX {
        if class == 0 || class > 32 {
            return false;
        }
        if metadata.allowable_class & (1u32 << (class - 1)) == 0 {
            return false;
        }
    }
    match metadata.item_class {
        2 => weapon_subclass_allowed(class, metadata.subclass),
        4 => armor_subclass_allowed(class, level, metadata.subclass),
        _ => !destination_slots(metadata).is_empty(),
    }
}

pub fn container_can_equip(metadata: &ItemTemplateMetadata, class: u8, level: u8) -> bool {
    metadata.inventory_type == 18
        && metadata.container_slots > 0
        && metadata.required_level <= u32::from(level)
        && (metadata.allowable_class == 0
            || metadata.allowable_class == u32::MAX
            || (class > 0 && class <= 32 && metadata.allowable_class & (1u32 << (class - 1)) != 0))
}

fn armor_subclass_allowed(class: u8, level: u8, subclass: u32) -> bool {
    match subclass {
        0 | 1 => true, // misc / cloth
        2 => matches!(class, 1 | 2 | 3 | 4 | 6 | 7 | 11),
        3 => matches!(class, 1 | 2) || (matches!(class, 3 | 7) && level >= 40),
        4 => matches!(class, 1 | 2 | 6) && (level >= 40 || class == 6),
        6 => matches!(class, 1 | 2 | 7), // shield
        7 => class == 2,                 // libram
        8 => class == 11,                // idol
        9 => class == 7,                 // totem
        10 => class == 6,                // sigil
        _ => false,
    }
}

fn weapon_subclass_allowed(class: u8, subclass: u32) -> bool {
    match class {
        1 => matches!(
            subclass,
            0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 10 | 13 | 15 | 16 | 18
        ),
        2 => matches!(subclass, 0 | 1 | 4 | 5 | 6 | 7 | 8),
        3 => matches!(subclass, 0 | 1 | 2 | 3 | 6 | 7 | 8 | 10 | 13 | 15 | 18),
        4 => matches!(subclass, 2 | 3 | 4 | 7 | 13 | 15 | 16 | 18),
        5 => matches!(subclass, 4 | 10 | 15 | 19),
        6 => matches!(subclass, 0 | 1 | 4 | 5 | 6 | 7 | 8),
        7 => matches!(subclass, 0 | 1 | 4 | 5 | 10 | 13 | 15),
        8 => matches!(subclass, 7 | 10 | 15 | 19),
        9 => matches!(subclass, 7 | 10 | 15 | 19),
        11 => matches!(subclass, 4 | 5 | 6 | 10 | 13 | 15),
        _ => false,
    }
}

pub fn item_score_with_spec(
    metadata: &ItemTemplateMetadata,
    class: u8,
    spec_tree: Option<u8>,
) -> f32 {
    let mut score = metadata.item_level as f32 * 0.35 + metadata.quality as f32 * 2.0;
    score += metadata.armor as f32 * armor_weight(class, spec_tree, metadata.inventory_type);

    for (stat_type, value) in &metadata.stats {
        score += *value as f32 * stat_weight(class, spec_tree, *stat_type);
    }

    if metadata.item_class == 2 && metadata.delay_ms > 0 {
        let average_damage = (metadata.damage_min + metadata.damage_max) * 0.5;
        let dps = average_damage * 1000.0 / metadata.delay_ms as f32;
        score += dps * weapon_weight(class, spec_tree, metadata.inventory_type);
    }
    score
}

/// Compare a usable item with its best observed equipment slot.
/// Missing equipment, class, level, or item-template facts stay unknown.
pub fn equipment_score_comparison(snapshot: &Snapshot, item_id: u32) -> Option<(f32, f32)> {
    let inventory = &snapshot.state.inventory;
    if !inventory.equipment_authoritative || !inventory.equipment_slots_authoritative {
        return None;
    }
    let metadata = inventory.item_metadata.get(&item_id)?;
    let slots = destination_slots(metadata);
    if slots.is_empty() {
        return None;
    }
    let class = snapshot.state.capabilities.class_id?;
    let player = snapshot
        .state
        .session
        .character_guid
        .map(EntityId)
        .and_then(|id| snapshot.state.entities.0.get(&id))?;
    let level = player.level?;
    if !player_can_use(metadata, class, level.min(255) as u8) {
        return None;
    }
    let new_score = item_score_with_spec(
        metadata,
        class,
        snapshot.state.capabilities.specialization_tree,
    );
    let mut current_scores = Vec::with_capacity(slots.len());
    for slot in slots {
        let mut current_score = if let Some(equipped_id) = inventory.equipped_items.get(slot) {
            let equipped = inventory.item_metadata.get(equipped_id)?;
            item_score_with_spec(
                equipped,
                class,
                snapshot.state.capabilities.specialization_tree,
            )
        } else {
            0.0
        };
        if metadata.inventory_type == 17
            && *slot == 15
            && let Some(offhand_id) = inventory.equipped_items.get(&16)
        {
            let offhand = inventory.item_metadata.get(offhand_id)?;
            current_score += item_score_with_spec(
                offhand,
                class,
                snapshot.state.capabilities.specialization_tree,
            );
        }
        current_scores.push(current_score);
    }
    let current_score = current_scores.into_iter().min_by(f32::total_cmp)?;
    Some((current_score, new_score))
}

/// Return whether an observed item is a usable equipment upgrade.
/// Missing equipment, class, level, or item-template facts stay unknown.
pub fn usable_equipment_upgrade(snapshot: &Snapshot, item_id: u32) -> Option<bool> {
    let metadata = snapshot.state.inventory.item_metadata.get(&item_id)?;
    if destination_slots(metadata).is_empty() {
        return Some(false);
    }
    let class = snapshot.state.capabilities.class_id?;
    let player = snapshot
        .state
        .session
        .character_guid
        .map(EntityId)
        .and_then(|id| snapshot.state.entities.0.get(&id))?;
    let level = player.level?;
    if !player_can_use(metadata, class, level.min(255) as u8) {
        return Some(false);
    }
    let (current, new) = equipment_score_comparison(snapshot, item_id)?;
    Some(score_improves_by_fraction(new, current, 0.01))
}

fn armor_weight(class: u8, spec_tree: Option<u8>, inventory_type: u32) -> f32 {
    if is_tank_spec(class, spec_tree) {
        if inventory_type == 14 { 0.16 } else { 0.07 }
    } else if inventory_type == 14 && matches!(class, 1 | 2 | 7) {
        0.08
    } else {
        0.025
    }
}

fn weapon_weight(class: u8, spec_tree: Option<u8>, inventory_type: u32) -> f32 {
    let ranged = matches!(inventory_type, 15 | 25 | 26);
    if is_healer_spec(class, spec_tree) {
        0.45
    } else if class == 3 && ranged {
        3.2
    } else if matches!(class, 1 | 2 | 4 | 6 | 7 | 11) && !ranged {
        2.7
    } else {
        1.2
    }
}

fn is_tank_spec(class: u8, spec_tree: Option<u8>) -> bool {
    matches!(
        (class, spec_tree),
        (1, Some(2)) | (2, Some(1)) | (6, Some(0)) | (11, Some(1))
    )
}

fn is_healer_spec(class: u8, spec_tree: Option<u8>) -> bool {
    matches!(
        (class, spec_tree),
        (2, Some(0)) | (5, Some(0 | 1)) | (7, Some(2)) | (11, Some(2))
    )
}

fn stat_weight(class: u8, spec_tree: Option<u8>, stat: i32) -> f32 {
    if let Some(weight) = spec_stat_weight(class, spec_tree, stat) {
        return weight;
    }
    // WotLK ItemModType values. Unknown stats get a small positive weight so
    // a higher-level usable item is not discarded only because its stat is new.
    match class {
        1 => match stat {
            4 => 1.8,
            7 => 1.2,
            32 | 36 | 37 | 38 | 44 => 1.0,
            3 => 0.6,
            _ => 0.15,
        },
        2 => match stat {
            4 | 5 => 1.4,
            7 => 1.1,
            32 | 36 | 37 | 38 | 45 => 0.9,
            _ => 0.15,
        },
        3 => match stat {
            3 => 1.8,
            7 => 1.0,
            32 | 36 | 39 | 44 => 1.1,
            5 => 0.4,
            _ => 0.15,
        },
        4 => match stat {
            3 => 1.9,
            7 => 0.8,
            32 | 36 | 37 | 38 | 44 => 1.0,
            _ => 0.1,
        },
        5 => match stat {
            5 => 1.7,
            6 | 7 => 0.8,
            32 | 36 | 43 | 45 => 1.1,
            _ => 0.1,
        },
        6 => match stat {
            4 => 1.7,
            7 => 1.2,
            32 | 36 | 37 | 38 => 1.0,
            _ => 0.1,
        },
        7 => match stat {
            5 => 1.4,
            4 | 3 => 1.0,
            7 => 1.1,
            32 | 36 | 43 | 45 => 0.9,
            _ => 0.15,
        },
        8 | 9 => match stat {
            5 => 1.8,
            7 => 0.8,
            32 | 36 | 43 | 45 => 1.1,
            6 => 0.4,
            _ => 0.1,
        },
        11 => match stat {
            3 | 5 => 1.4,
            4 => 0.9,
            7 => 1.0,
            32 | 36 | 43 | 45 => 0.9,
            _ => 0.15,
        },
        _ => 0.1,
    }
}

fn spec_stat_weight(class: u8, spec_tree: Option<u8>, stat: i32) -> Option<f32> {
    let tree = spec_tree?;
    let weight = match (class, tree) {
        // Paladin: Holy / Protection / Retribution.
        (2, 0) => match stat {
            5 => 2.0,
            6 => 0.7,
            7 => 0.7,
            32 | 36 | 43 | 45 => 1.2,
            4 => 0.2,
            _ => 0.12,
        },
        (2, 1) => match stat {
            7 => 1.8,
            4 => 1.2,
            12 | 13 | 14 | 15 | 16 | 32 => 1.1,
            5 => 0.5,
            _ => 0.12,
        },
        (2, 2) => match stat {
            4 => 2.0,
            7 => 0.9,
            32 | 36 | 37 | 38 | 44 => 1.2,
            5 => 0.25,
            _ => 0.12,
        },
        // Priest: healing trees favor intellect/spirit; Shadow favors caster throughput.
        (5, 0 | 1) => match stat {
            5 => 1.9,
            6 => 1.1,
            7 => 0.7,
            32 | 36 | 43 | 45 => 1.2,
            _ => 0.1,
        },
        (5, 2) => match stat {
            5 => 1.7,
            6 => 0.7,
            32 | 36 | 43 | 45 => 1.35,
            7 => 0.6,
            _ => 0.1,
        },
        // Shaman: Elemental / Enhancement / Restoration.
        (7, 0) => match stat {
            5 => 1.8,
            32 | 36 | 43 | 45 => 1.35,
            7 => 0.7,
            3 | 4 => 0.2,
            _ => 0.1,
        },
        (7, 1) => match stat {
            3 => 1.8,
            4 => 1.5,
            7 => 0.9,
            32 | 36 | 37 | 38 | 44 => 1.2,
            5 => 0.25,
            _ => 0.1,
        },
        (7, 2) => match stat {
            5 => 2.0,
            6 => 0.8,
            32 | 36 | 43 | 45 => 1.15,
            7 => 0.7,
            3 | 4 => 0.15,
            _ => 0.1,
        },
        // Druid: Balance / Feral Bear / Restoration. The embedded Playerbots
        // Feral PvE template is the bear build, so defensive stats matter.
        (11, 0) => match stat {
            5 => 1.8,
            6 => 0.7,
            32 | 36 | 43 | 45 => 1.3,
            3 | 4 => 0.2,
            7 => 0.7,
            _ => 0.1,
        },
        (11, 1) => match stat {
            7 => 1.9,
            3 => 1.5,
            4 => 0.9,
            12..=16 => 1.25,
            32 | 36 | 37 | 38 | 44 => 0.8,
            5 => 0.1,
            _ => 0.1,
        },
        (11, 2) => match stat {
            5 => 1.8,
            6 => 1.4,
            32 | 36 | 43 | 45 => 1.15,
            7 => 0.7,
            3 | 4 => 0.15,
            _ => 0.1,
        },
        // Blood is the tank-oriented Death Knight template used by role logic.
        (6, 0) => match stat {
            7 => 1.9,
            4 => 1.3,
            12..=16 => 1.25,
            32 | 36 | 37 | 38 => 0.85,
            _ => 0.12,
        },
        // Warrior Protection has materially different gear goals.
        (1, 2) => match stat {
            7 => 2.0,
            4 => 1.1,
            12..=16 => 1.35,
            32 | 36 | 37 | 38 => 0.7,
            _ => 0.12,
        },
        _ => return None,
    };
    Some(weight)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::EntityId;
    use wow_state::{
        AuthoritativeState, Snapshot,
        entities::{EntityKind, EntityState},
    };

    fn item(inventory_type: u32, item_level: u32, stat: (i32, i32)) -> ItemTemplateMetadata {
        ItemTemplateMetadata {
            name: String::new(),
            item_class: 4,
            subclass: 2,
            quality: 2,
            sell_price: 1,
            inventory_type,
            allowable_class: u32::MAX,
            item_level,
            required_level: 1,
            container_slots: 0,
            stats: vec![stat],
            armor: item_level * 2,
            damage_min: 0.0,
            damage_max: 0.0,
            delay_ms: 0,
            ammo_type: 0,
            use_spell_id: 0,
        }
    }

    #[test]
    fn fractional_upgrade_threshold_requires_a_meaningful_improvement() {
        assert!(!score_improves_by_fraction(10.49, 10.0, 0.05));
        assert!(score_improves_by_fraction(10.51, 10.0, 0.05));
        assert!(!score_improves_by_fraction(0.05, 0.0, 0.05));
        assert!(score_improves_by_fraction(0.051, 0.0, 0.05));
        assert!(score_improves_by_fraction(10.01, 10.0, -1.0));
    }

    #[test]
    fn loot_upgrade_evidence_requires_authoritative_equipment_and_known_templates() {
        let mut state = AuthoritativeState::default();
        state.session.character_guid = Some(1);
        state.capabilities.class_id = Some(1);
        state.capabilities.specialization_tree = Some(0);
        state.inventory.equipment_authoritative = true;
        state.inventory.equipment_slots_authoritative = true;
        state.inventory.equipped_items.insert(0, 100);
        state
            .inventory
            .item_metadata
            .insert(100, item(1, 10, (4, 2)));
        state
            .inventory
            .item_metadata
            .insert(200, item(1, 80, (4, 20)));
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                level: Some(60),
                ..EntityState::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(usable_equipment_upgrade(&snapshot, 200), Some(true));

        state.inventory.item_metadata.remove(&100);
        assert_eq!(
            usable_equipment_upgrade(&Snapshot::from_state(&state), 200),
            None
        );
        state.inventory.equipment_slots_authoritative = false;
        assert_eq!(
            usable_equipment_upgrade(&Snapshot::from_state(&state), 200),
            None
        );
    }

    #[test]
    fn warrior_values_strength_more_than_intellect() {
        let strength = item(1, 20, (4, 10));
        let intellect = item(1, 20, (5, 10));
        assert!(
            item_score_with_spec(&strength, 1, Some(0))
                > item_score_with_spec(&intellect, 1, Some(0))
        );
    }

    #[test]
    fn ring_and_trinket_have_two_destination_slots() {
        assert_eq!(destination_slots(&item(11, 10, (4, 1))), &[10, 11]);
        assert_eq!(destination_slots(&item(12, 10, (4, 1))), &[12, 13]);
    }

    #[test]
    fn class_and_level_restrictions_fail_closed() {
        let mut plate = item(5, 40, (4, 10));
        plate.subclass = 4;
        plate.required_level = 40;
        assert!(!player_can_use(&plate, 1, 39));
        assert!(player_can_use(&plate, 1, 40));
        assert!(!player_can_use(&plate, 8, 80));
    }

    #[test]
    fn spec_changes_gear_priority() {
        let stamina = item(5, 60, (7, 20));
        let intellect = item(5, 60, (5, 20));
        assert!(
            item_score_with_spec(&stamina, 2, Some(1))
                > item_score_with_spec(&intellect, 2, Some(1))
        );
        assert!(
            item_score_with_spec(&intellect, 2, Some(0))
                > item_score_with_spec(&stamina, 2, Some(0))
        );
    }
    #[test]
    fn tank_spec_detection_matches_role_profiles() {
        assert!(is_tank_spec(1, Some(2)));
        assert!(is_tank_spec(2, Some(1)));
        assert!(is_tank_spec(6, Some(0)));
        assert!(is_tank_spec(11, Some(1)));
        assert!(!is_tank_spec(8, Some(0)));
    }

    #[test]
    fn shaman_spec_changes_primary_stat_preference() {
        let agility = item(1, 30, (3, 10));
        let intellect = item(1, 30, (5, 10));
        assert!(
            item_score_with_spec(&agility, 7, Some(1))
                > item_score_with_spec(&intellect, 7, Some(1))
        );
        assert!(
            item_score_with_spec(&intellect, 7, Some(2))
                > item_score_with_spec(&agility, 7, Some(2))
        );
    }
}
