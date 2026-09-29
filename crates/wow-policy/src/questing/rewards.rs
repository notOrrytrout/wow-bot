use wow_domain::EntityId;
use wow_state::{Snapshot, inventory::ItemTemplateMetadata};

const REWARD_METADATA_WAIT: std::time::Duration = std::time::Duration::from_secs(8);

pub fn metadata_wait() -> std::time::Duration {
    REWARD_METADATA_WAIT
}

/// Return reward and equipped-item templates that are needed for a complete
/// comparison. Unknown reward templates are queried first; their equipment
/// slots identify any equipped templates needed on the next pass.
pub fn missing_score_metadata(snapshot: &Snapshot, reward_items: &[u32]) -> Vec<u32> {
    let inventory = &snapshot.state.inventory;
    let mut missing = Vec::new();
    for item in reward_items.iter().copied().filter(|item| *item != 0) {
        let Some(metadata) = inventory.item_metadata.get(&item) else {
            missing.push(item);
            continue;
        };
        for slot in crate::gear::destination_slots(metadata) {
            if let Some(equipped) = inventory.equipped_items.get(slot)
                && !inventory.item_metadata.contains_key(equipped)
            {
                missing.push(*equipped);
            }
        }
    }
    missing.sort_unstable();
    missing.dedup();
    missing
}

/// Select the strongest authoritative equipment upgrade. When no reward can
/// be proven to improve an equipped slot, select the best known fallback by
/// vendor value and item level. The result is the server's zero-based choice
/// index, not an item id.
pub fn best_reward_index(snapshot: &Snapshot, reward_items: &[u32]) -> u32 {
    if reward_items.len() <= 1 {
        return 0;
    }
    let inventory = &snapshot.state.inventory;
    let player_id = snapshot.state.session.character_guid.map(EntityId);
    let player = player_id.and_then(|id| snapshot.state.entities.0.get(&id));
    let class = snapshot.state.capabilities.class_id.unwrap_or_default();
    let level = player.and_then(|entity| entity.level).unwrap_or_default();
    let spec = snapshot.state.capabilities.specialization_tree;
    let mut best_upgrade: Option<(usize, f32)> = None;
    let mut best_fallback: Option<(usize, f32)> = None;

    for (index, item_id) in reward_items.iter().copied().enumerate() {
        let Some(metadata) = inventory.item_metadata.get(&item_id) else {
            continue;
        };
        let fallback = metadata.sell_price as f32 + metadata.item_level as f32 * 10.0;
        if best_fallback.is_none_or(|(_, score)| fallback > score) {
            best_fallback = Some((index, fallback));
        }
        if class == 0
            || level == 0
            || !crate::gear::player_can_use(metadata, class, level.min(255) as u8)
        {
            continue;
        }
        let new_score = crate::gear::item_score_with_spec(metadata, class, spec);
        let Some(current_score) = current_score(snapshot, metadata, class, spec) else {
            continue;
        };
        let delta = new_score - current_score;
        if crate::gear::score_improves_by_fraction(new_score, current_score, 0.01)
            && best_upgrade.is_none_or(|(_, score)| delta > score)
        {
            best_upgrade = Some((index, delta));
        }
    }

    best_upgrade
        .or(best_fallback)
        .map_or(0, |(index, _)| index as u32)
}

fn current_score(
    snapshot: &Snapshot,
    reward: &ItemTemplateMetadata,
    class: u8,
    spec: Option<u8>,
) -> Option<f32> {
    let inventory = &snapshot.state.inventory;
    let slots = crate::gear::destination_slots(reward);
    if slots.is_empty() {
        return None;
    }
    slots
        .iter()
        .filter_map(|slot| {
            let item = inventory.equipped_items.get(slot)?;
            let metadata = inventory.item_metadata.get(item)?;
            let mut score = crate::gear::item_score_with_spec(metadata, class, spec);
            if reward.inventory_type == 17
                && *slot == 15
                && let Some(offhand) = inventory.equipped_items.get(&16)
                && let Some(offhand_metadata) = inventory.item_metadata.get(offhand)
            {
                score += crate::gear::item_score_with_spec(offhand_metadata, class, spec);
            }
            Some(score)
        })
        .min_by(f32::total_cmp)
        .or(Some(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{EntityId, WorldPosition};
    use wow_state::{
        AuthoritativeState,
        entities::{EntityKind, EntityState},
    };

    fn metadata(item_level: u32, stat_value: i32) -> ItemTemplateMetadata {
        ItemTemplateMetadata {
            item_class: 4,
            subclass: 1,
            quality: 2,
            sell_price: item_level,
            inventory_type: 1,
            allowable_class: u32::MAX,
            item_level,
            required_level: 1,
            stats: vec![(4, stat_value)],
            armor: item_level,
            ..Default::default()
        }
    }

    fn snapshot() -> Snapshot {
        let mut state = AuthoritativeState::default();
        state.session.character_guid = Some(1);
        state.capabilities.class_id = Some(1);
        state.capabilities.specialization_tree = Some(0);
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                level: Some(60),
                position: Some(WorldPosition::default()),
                ..Default::default()
            },
        );
        state.inventory.equipped_items.insert(0, 100);
        state.inventory.item_metadata.insert(100, metadata(10, 1));
        Snapshot::from_state(&state)
    }

    #[test]
    fn selects_the_strongest_usable_upgrade() {
        let mut snapshot = snapshot();
        snapshot
            .state
            .inventory
            .item_metadata
            .insert(200, metadata(20, 15));
        snapshot
            .state
            .inventory
            .item_metadata
            .insert(201, metadata(18, 10));
        assert_eq!(best_reward_index(&snapshot, &[201, 200]), 1);
    }

    #[test]
    fn fallback_uses_item_value_when_no_reward_is_an_upgrade() {
        let mut snapshot = snapshot();
        snapshot
            .state
            .inventory
            .item_metadata
            .insert(200, metadata(8, 1));
        snapshot
            .state
            .inventory
            .item_metadata
            .insert(201, metadata(9, 1));
        assert_eq!(best_reward_index(&snapshot, &[200, 201]), 1);
    }

    #[test]
    fn missing_reward_and_equipped_templates_are_reported() {
        let mut snapshot = snapshot();
        snapshot.state.inventory.item_metadata.remove(&100);
        assert_eq!(missing_score_metadata(&snapshot, &[200]), vec![200]);
        snapshot
            .state
            .inventory
            .item_metadata
            .insert(200, metadata(20, 15));
        assert_eq!(missing_score_metadata(&snapshot, &[200]), vec![100]);
    }
}
