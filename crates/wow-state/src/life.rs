use serde::{Deserialize, Serialize};
use wow_domain::WorldPosition;

/// Ghost aura applied by the WotLK server when a player releases their spirit.
pub const GHOST_AURA_SPELL_ID: u32 = 8326;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LifeState {
    pub corpse: Option<WorldPosition>,
    pub reclaim_ready_at_ms: u64,
    pub recovery_generation: u64,
}
