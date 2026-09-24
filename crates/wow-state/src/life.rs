use serde::{Deserialize, Serialize};
use wow_domain::WorldPosition;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LifeState {
    pub corpse: Option<WorldPosition>,
    pub reclaim_ready_at_ms: u64,
    pub recovery_generation: u64,
}
