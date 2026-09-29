use crate::entities::Entities;
use serde::{Deserialize, Serialize};
use wow_domain::EntityId;

/// Server-reported pet ability and autocast state from SMSG_PET_SPELLS.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PetAbilityState {
    pub spell: u32,
    /// `None` means the ability is passive or the server did not report its autocast state.
    #[serde(default)]
    pub autocast: Option<bool>,
}

/// Server-confirmed state for the player's current controllable pet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PetState {
    /// False until the server confirms pet presence or absence.
    #[serde(default)]
    pub control_known: bool,
    #[serde(default)]
    pub guid: Option<EntityId>,
    /// WotLK reaction state: 0 passive, 1 defensive, 2 aggressive.
    #[serde(default)]
    pub reaction: Option<u8>,
    /// Pet spell-bar state. It is authoritative after SMSG_PET_SPELLS.
    #[serde(default)]
    pub abilities: Vec<PetAbilityState>,
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
