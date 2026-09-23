use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use wow_domain::{EntityId, WorldPosition};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ControlState {
    /// Current server-authoritative mover controlled by the player. `None` means
    /// normal player movement. This is populated from UNIT_FIELD_CHARM and
    /// related authoritative object state.
    pub mover: Option<EntityId>,
    pub mover_position: Option<WorldPosition>,
    pub movement_flags: u32,
    /// Abilities exposed by the controlled-unit/vehicle action bar.
    pub abilities: BTreeSet<u32>,
}

impl ControlState {
    pub fn active_position(&self, player: Option<WorldPosition>) -> Option<WorldPosition> {
        self.mover_position.or(player)
    }
}
