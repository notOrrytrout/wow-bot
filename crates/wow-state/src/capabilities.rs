use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CapabilityState {
    pub class_id: Option<u8>,
    pub spells: BTreeSet<u32>,
    pub spell_cooldowns: BTreeMap<u32, u64>,
    pub items: BTreeSet<u32>,
    pub can_fish: bool,
    pub mounted: bool,
}
