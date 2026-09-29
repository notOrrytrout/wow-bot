use serde::{Deserialize, Serialize};
use wow_domain::EntityId;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainerSpellOffer {
    pub spell: u32,
    /// WotLK uses zero for an offer the player can learn now.
    pub usable: u8,
    pub cost_copper: u32,
    pub required_level: u8,
    pub required_skill_line: u32,
    pub required_skill_rank: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainerState {
    pub trainer: Option<EntityId>,
    pub trainer_type: Option<i32>,
    pub offers: Vec<TrainerSpellOffer>,
}
