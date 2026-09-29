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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EquippedItemInstance {
    pub item: u32,
    pub guid: EntityId,
    /// None means the item update did not prove whether its temporary enchant is active.
    #[serde(default)]
    pub temporary_enchanted: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemTemplateMetadata {
    #[serde(default)]
    pub name: String,
    pub item_class: u32,
    #[serde(default)]
    pub subclass: u32,
    pub quality: u32,
    pub sell_price: u32,
    #[serde(default)]
    pub inventory_type: u32,
    #[serde(default)]
    pub allowable_class: u32,
    #[serde(default)]
    pub item_level: u32,
    #[serde(default)]
    pub required_level: u32,
    #[serde(default)]
    pub stats: Vec<(i32, i32)>,
    #[serde(default)]
    pub armor: u32,
    #[serde(default)]
    pub damage_min: f32,
    #[serde(default)]
    pub damage_max: f32,
    #[serde(default)]
    pub delay_ms: u32,
    #[serde(default)]
    pub container_slots: u32,
    #[serde(default)]
    pub ammo_type: u32,
    #[serde(default)]
    pub use_spell_id: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EquipmentCondition {
    #[serde(default)]
    pub observed: bool,
    #[serde(default)]
    pub lowest_durability_percent: Option<u8>,
    #[serde(default)]
    pub broken_items: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorOffer {
    pub slot: u32,
    pub item: u32,
    pub stock: Option<u32>,
    pub price_copper: u32,
    /// Number of items granted by one purchase lot.
    pub buy_count: u32,
    pub extended_cost: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorInventory {
    pub vendor: EntityId,
    pub offers: Vec<VendorOffer>,
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
    /// `None` means the protocol observation did not prove that this mail is non-COD.
    #[serde(default)]
    pub cod_copper: Option<u64>,
    pub attachments: BTreeMap<u32, MailAttachment>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MailAttachment {
    pub item_id: u32,
    pub count: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MailboxState {
    pub generation: u64,
    #[serde(default)]
    pub authoritative: bool,
    pub mails: BTreeMap<u32, MailEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InventoryState {
    pub items: BTreeMap<u32, u32>,
    pub instances: BTreeMap<EntityId, InventoryItemInstance>,
    #[serde(default)]
    pub instances_authoritative: bool,
    #[serde(default)]
    pub item_metadata: BTreeMap<u32, ItemTemplateMetadata>,
    pub free_slots: u16,
    pub equipped_ranged_item: Option<u32>,
    pub equipped_items: BTreeMap<u8, u32>,
    #[serde(default)]
    pub equipped_item_instances: BTreeMap<u8, EquippedItemInstance>,
    pub equipment_slots_authoritative: bool,
    pub equipment_authoritative: bool,
    #[serde(default)]
    pub equipment_condition: EquipmentCondition,
    pub money: u64,
    pub loot_generation: u64,
    pub bot_loot_generation: u64,
    pub current_loot: Option<EntityId>,
    pub current_loot_owner: Option<crate::observation::LootOwnership>,
    pub vendor: Option<EntityId>,
    #[serde(default)]
    pub vendor_inventory: Option<VendorInventory>,
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
