use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wow_domain::EntityId;

/// Return whether an AzerothCore 3.3.5a quest-giver status has an available quest icon.
pub const fn quest_giver_has_available_quest(status: u8) -> bool {
    matches!(status, 2 | 4 | 7 | 8)
}

/// Return whether an AzerothCore 3.3.5a quest-giver status has a reward icon.
pub const fn quest_giver_has_reward(status: u8) -> bool {
    matches!(status, 3 | 6 | 9 | 10)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuestProgress {
    pub complete: bool,
    pub objectives: Vec<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuestOffer {
    pub giver: EntityId,
    pub icon: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum QuestTurnInStage {
    RequestItems { can_complete: bool },
    OfferReward { reward_items: Vec<u32> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuestTurnInDialog {
    pub giver: EntityId,
    pub stage: QuestTurnInStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum QuestTargetKind {
    Creature,
    GameObject,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuestTargetObjective {
    pub slot: usize,
    pub kind: QuestTargetKind,
    pub entry: u32,
    pub required: u32,
    pub item_drop: u32,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuestItemObjective {
    pub item: u32,
    pub required: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuestDefinition {
    pub quest: u32,
    pub title: String,
    pub poi_map: Option<u32>,
    pub poi_x: Option<f32>,
    pub poi_y: Option<f32>,
    pub targets: Vec<QuestTargetObjective>,
    pub items: Vec<QuestItemObjective>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuestState {
    pub active: BTreeMap<u32, QuestProgress>,
    pub completed: Vec<u32>,
    pub giver_status: BTreeMap<EntityId, u8>,
    pub offers: BTreeMap<u32, QuestOffer>,
    pub definitions: BTreeMap<u32, QuestDefinition>,
    pub turn_in: BTreeMap<u32, QuestTurnInDialog>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quest_giver_dialog_statuses_match_azerothcore_335a() {
        for status in [2, 4, 7, 8] {
            assert!(quest_giver_has_available_quest(status));
        }
        for status in [3, 6, 9, 10] {
            assert!(quest_giver_has_reward(status));
        }
        for status in [0, 1, 5, 11, u8::MAX] {
            assert!(!quest_giver_has_available_quest(status));
            assert!(!quest_giver_has_reward(status));
        }
    }
}
