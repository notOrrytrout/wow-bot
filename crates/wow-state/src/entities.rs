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
    pub target: Option<EntityId>,
    pub hostile: bool,
    pub interactable: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Entities(pub BTreeMap<EntityId, EntityState>);
