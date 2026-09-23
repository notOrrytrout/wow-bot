use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProfessionState {
    pub skills: BTreeMap<u32, (u16, u16)>,
    pub known_recipes: BTreeSet<u32>,
    pub primary_professions: BTreeSet<u32>,
    pub cooking: bool,
    pub first_aid: bool,
    pub fishing: bool,
}
impl ProfessionState {
    pub fn skill(&self, skill: u32) -> u16 { self.skills.get(&skill).map_or(0, |v| v.0) }
}
