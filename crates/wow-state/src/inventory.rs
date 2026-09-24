use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wow_domain::EntityId;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: u32,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryItemInstance {
    pub item: u32,
    pub guid: EntityId,
    pub backpack_slot: u8,
    pub count: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TradeState {
    pub generation: u64,
    pub partner: Option<EntityId>,
    pub our_items: BTreeMap<u32, u32>,
    pub their_items: BTreeMap<u32, u32>,
    pub our_money: u64,
    pub their_money: u64,
    pub open: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuctionListing {
    pub listing_id: u64,
    pub item: u32,
    pub count: u32,
    pub buyout: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuctionState {
    pub query_generation: u64,
    pub listings: BTreeMap<u64, AuctionListing>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MailEntry {
    pub mail_id: u32,
    pub money: u64,
    pub attachments: BTreeMap<u32, u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MailboxState {
    pub generation: u64,
    pub mails: BTreeMap<u32, MailEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InventoryState {
    pub items: BTreeMap<u32, u32>,
    pub instances: BTreeMap<EntityId, InventoryItemInstance>,
    pub free_slots: u16,
    pub equipped_ranged_item: Option<u32>,
    pub equipped_items: BTreeMap<u8, u32>,
    pub equipment_slots_authoritative: bool,
    pub equipment_authoritative: bool,
    pub money: u64,
    pub loot_generation: u64,
    pub bot_loot_generation: u64,
    pub current_loot: Option<EntityId>,
    pub current_loot_owner: Option<crate::observation::LootOwnership>,
    pub vendor: Option<EntityId>,
    pub trade: TradeState,
    pub auction: AuctionState,
    pub mailbox: MailboxState,
}

impl InventoryState {
    pub fn count(&self, item: u32) -> u32 {
        self.items.get(&item).copied().unwrap_or_default()
    }

    pub fn has(&self, item: u32, count: u32) -> bool {
        self.count(item) >= count
    }

    pub fn usable_instance(&self, item: u32) -> Option<&InventoryItemInstance> {
        self.instances
            .values()
            .filter(|instance| instance.item == item && instance.count > 0)
            .min_by_key(|instance| (instance.backpack_slot, instance.guid))
    }
}

#[cfg(test)]
mod tests {
    use super::InventoryState;

    #[test]
    fn item_count_is_shared_by_presence_checks() {
        let mut inventory = InventoryState::default();
        assert_eq!(inventory.count(7), 0);
        inventory.items.insert(7, 3);
        assert_eq!(inventory.count(7), 3);
        assert!(inventory.has(7, 3));
        assert!(!inventory.has(7, 4));
    }
}
