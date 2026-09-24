use crate::{EntityId, Vec3};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StrategicIntent {
    Idle,
    MoveTo(Vec3),
    Attack(EntityId),
    Interact(EntityId),
    Cast {
        spell: u32,
        target: Option<EntityId>,
    },
    Loot(EntityId),
    Fish,
    UseItem {
        item: u32,
        target: Option<EntityId>,
    },
    Custom {
        kind: String,
        payload: String,
    },
}
