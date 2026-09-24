use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TalentRank {
    pub talent_id: u32,
    /// WotLK sends talent rank as zero based; convert to points when ranking trees.
    pub rank: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CapabilityState {
    pub class_id: Option<u8>,
    pub talent_group_count: Option<u8>,
    pub active_talent_group: Option<u8>,
    pub active_talents: Vec<TalentRank>,
    pub specialization_tree: Option<u8>,
    pub spells: BTreeSet<u32>,
    pub spell_cooldowns: BTreeMap<u32, u64>,
    pub items: BTreeSet<u32>,
    pub can_fish: bool,
    pub mounted: bool,
}
