use crate::{MissionId, PermissionSet};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GroupRole { Tank, Healer, Melee, Ranged, Support }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MissionIntent {
    Idle,
    Quest,
    Gather { resource: String },
    Grind { creature: String },
    Battleground { battleground: Option<String> },
    Party { role: GroupRole },
    Raid { role: GroupRole },
    Goal { text: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Mission {
    pub id: MissionId,
    pub intent: MissionIntent,
    pub permissions: PermissionSet,
}

impl Mission {
    pub fn idle() -> Self { Self { id: MissionId(0), intent: MissionIntent::Idle, permissions: PermissionSet::MAINTENANCE } }
    pub fn quest(id: MissionId) -> Self { Self { id, intent: MissionIntent::Quest, permissions: PermissionSet::MOVE | PermissionSet::QUEST | PermissionSet::COMBAT | PermissionSet::LOOT | PermissionSet::MAINTENANCE } }
    pub fn gather(id: MissionId, resource: impl Into<String>) -> Self { Self { id, intent: MissionIntent::Gather { resource: resource.into() }, permissions: PermissionSet::MOVE | PermissionSet::GATHER | PermissionSet::COMBAT | PermissionSet::LOOT | PermissionSet::MAINTENANCE } }
    pub fn grind(id: MissionId, creature: impl Into<String>) -> Self { Self { id, intent: MissionIntent::Grind { creature: creature.into() }, permissions: PermissionSet::MOVE | PermissionSet::COMBAT | PermissionSet::LOOT | PermissionSet::MAINTENANCE } }
    pub fn goal(id: MissionId, text: impl Into<String>) -> Self { Self { id, intent: MissionIntent::Goal { text: text.into() }, permissions: PermissionSet::MOVE | PermissionSet::COMBAT | PermissionSet::LOOT | PermissionSet::QUEST | PermissionSet::GATHER | PermissionSet::MAINTENANCE } }
}
