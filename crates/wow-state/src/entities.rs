use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wow_domain::{EntityId, WorldPosition};
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum EntityKind {
    #[default]
    Unknown,
    Unit,
    Player,
    GameObject,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    pub id: EntityId,
    pub entry: u32,
    pub kind: EntityKind,
    pub name: Option<String>,
    pub position: Option<WorldPosition>,
    pub health: Option<(u32, u32)>,
    pub power: Option<(u32, u32)>,
    pub power_type: Option<u8>,
    pub level: Option<u32>,
    pub base_health: Option<u32>,
    pub base_mana: Option<u32>,
    pub power_cost_modifiers: Option<[i32; 7]>,
    pub power_cost_multipliers: Option<[f32; 7]>,
    pub base_attack_time_ms: Option<[u32; 2]>,
    pub shapeshift_form: Option<u8>,
    pub aura_state: Option<u32>,
    pub target: Option<EntityId>,
    pub hostile: bool,
    pub interactable: bool,
}

impl EntityState {
    /// Return true only when observed health confirms that the entity is dead.
    pub fn is_dead(&self) -> bool {
        self.health.is_some_and(|(current, _)| current == 0)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Entities(pub BTreeMap<EntityId, EntityState>);
