use crate::AuthoritativeState;
use serde::{Deserialize, Serialize};
use wow_domain::{StateRevision, WorldPosition};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: StateRevision,
    pub state: AuthoritativeState,
}
impl Snapshot {
    pub fn from_state(state: &AuthoritativeState) -> Self {
        Self {
            revision: state.revision,
            state: state.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SanitizedSnapshot {
    pub revision: StateRevision,
    pub in_world: bool,
    pub position: Option<WorldPosition>,
    pub nearby_entities: usize,
    pub active_quests: usize,
    pub free_slots: u16,
}
impl From<&Snapshot> for SanitizedSnapshot {
    fn from(s: &Snapshot) -> Self {
        Self {
            revision: s.revision,
            in_world: s.state.session.in_world,
            position: s.state.position.player,
            nearby_entities: s.state.entities.0.len(),
            active_quests: s.state.quests.active.len(),
            free_slots: s.state.inventory.free_slots,
        }
    }
}
