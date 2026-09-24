use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use wow_domain::EntityId;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuraInstance {
    pub slot: u8,
    pub spell: u32,
    pub positive: Option<bool>,
    pub caster: Option<EntityId>,
    pub max_duration_ms: Option<u32>,
    pub remaining_ms: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuraState {
    pub by_entity: BTreeMap<EntityId, BTreeMap<u8, AuraInstance>>,
}

impl AuraState {
    pub fn spells(&self, entity: EntityId) -> BTreeSet<u32> {
        self.by_entity
            .get(&entity)
            .into_iter()
            .flat_map(|slots| slots.values())
            .map(|a| a.spell)
            .collect()
    }

    pub fn has_any(&self, entity: EntityId, spells: &[u32]) -> bool {
        self.by_entity
            .get(&entity)
            .is_some_and(|slots| slots.values().any(|aura| spells.contains(&aura.spell)))
    }
}
