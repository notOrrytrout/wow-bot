use crate::entities::Entities;
use serde::{Deserialize, Serialize};
use wow_domain::EntityId;

/// Server-confirmed state for the player's current controllable pet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PetState {
    /// False until the server confirms pet presence or absence.
    #[serde(default)]
    pub control_known: bool,
    #[serde(default)]
    pub guid: Option<EntityId>,
}

impl PetState {
    /// `None` means the server has not confirmed pet presence or absence.
    pub fn has_active_pet(&self, entities: &Entities) -> Option<bool> {
        if !self.control_known {
            return None;
        }
        let Some(guid) = self.guid else {
            return Some(false);
        };
        Some(!entities.0.get(&guid).is_some_and(|entity| entity.is_dead()))
    }
}
