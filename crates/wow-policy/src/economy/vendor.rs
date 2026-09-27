use wow_domain::EntityId;
use wow_state::Snapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VendorTrashCandidate {
    pub item: u32,
    pub guid: EntityId,
    pub count: u32,
    pub unit_sell_price: u32,
}

/// Return only known gray items that are safe to sell. Unknown items and
/// protected item classes remain in the bags.
pub fn safe_gray_item_sales(state: &Snapshot) -> Vec<VendorTrashCandidate> {
    let quest_items: std::collections::BTreeSet<u32> = state
        .state
        .quests
        .active
        .keys()
        .filter_map(|quest| state.state.quests.definitions.get(quest))
        .flat_map(|quest| quest.items.iter().map(|objective| objective.item))
        .collect();
    state
        .state
        .inventory
        .instances
        .values()
        .filter_map(|instance| {
            let metadata = state.state.inventory.item_metadata.get(&instance.item)?;
            let protected_class = matches!(metadata.item_class, 0 | 1 | 5 | 7 | 12 | 13);
            if instance.count == 0
                || protected_class
                || metadata.quality != 0
                || metadata.sell_price == 0
                || quest_items.contains(&instance.item)
                || instance.item == 6948
            {
                return None;
            }
            Some(VendorTrashCandidate {
                item: instance.item,
                guid: instance.guid,
                count: instance.count,
                unit_sell_price: metadata.sell_price,
            })
        })
        .collect()
}

pub fn vendor_interaction_valid(state: &Snapshot, vendor: EntityId) -> bool {
    state.state.inventory.vendor == Some(vendor)
        && state
            .state
            .entities
            .0
            .get(&vendor)
            .is_some_and(|e| e.interactable)
}
