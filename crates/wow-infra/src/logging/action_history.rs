use serde::{Deserialize, Serialize};
use wow_domain::{ActionId, LaneId, MissionRevision, PermissionRevision, TaskId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionHistoryEntry {
    pub lane: LaneId,
    pub task: TaskId,
    pub action: ActionId,
    pub mission_revision: MissionRevision,
    pub permission_revision: PermissionRevision,
    pub terminal: Option<String>,
    pub unix_ms: u64,
}
