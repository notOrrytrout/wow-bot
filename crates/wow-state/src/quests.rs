use serde::{Deserialize,Serialize};
use std::collections::BTreeMap;
use wow_domain::EntityId;

#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct QuestProgress { pub complete: bool, pub objectives: Vec<u32> }

#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct QuestOffer { pub giver: EntityId, pub icon: u32 }

#[derive(Clone,Copy,Debug,Eq,PartialEq,Serialize,Deserialize)]
pub enum QuestTurnInStage { RequestItems { can_complete: bool }, OfferReward { reward_choices: u32 } }

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct QuestTurnInDialog { pub giver: EntityId, pub stage: QuestTurnInStage }

#[derive(Clone,Copy,Debug,Eq,PartialEq,Serialize,Deserialize)]
pub enum QuestTargetKind { Creature, GameObject }

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct QuestTargetObjective {
    pub slot: usize,
    pub kind: QuestTargetKind,
    pub entry: u32,
    pub required: u32,
    pub item_drop: u32,
    pub text: String,
}

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct QuestItemObjective { pub item: u32, pub required: u32 }

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct QuestDefinition {
    pub quest: u32,
    pub title: String,
    pub poi_map: Option<u32>,
    pub poi_x: Option<f32>,
    pub poi_y: Option<f32>,
    pub targets: Vec<QuestTargetObjective>,
    pub items: Vec<QuestItemObjective>,
}

#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct QuestState {
    pub active: BTreeMap<u32, QuestProgress>,
    pub completed: Vec<u32>,
    pub giver_status: BTreeMap<EntityId, u8>,
    pub offers: BTreeMap<u32, QuestOffer>,
    pub definitions: BTreeMap<u32, QuestDefinition>,
    pub turn_in: BTreeMap<u32, QuestTurnInDialog>,
}
