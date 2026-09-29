use crate::{
    auras::AuraState, battleground::BattlegroundState, capabilities::CapabilityState,
    control::ControlState, desync::DesyncState, entities::Entities, group::GroupState,
    inventory::InventoryState, life::LifeState, pets::PetState, position::PositionState,
    professions::ProfessionState, quests::QuestState, session::SessionState, trainer::TrainerState,
    transport::TransportState,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wow_domain::{EntityId, StateRevision};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveCastState {
    pub spell: u32,
    pub started_at_ms: u64,
    pub ends_at_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthoritativeState {
    pub auras: AuraState,
    #[serde(default)]
    pub battleground: BattlegroundState,
    pub revision: StateRevision,
    pub session: SessionState,
    pub position: PositionState,
    #[serde(default)]
    pub transport: TransportState,
    pub entities: Entities,
    pub inventory: InventoryState,
    pub life: LifeState,
    pub quests: QuestState,
    pub professions: ProfessionState,
    #[serde(default)]
    pub trainer: TrainerState,
    pub pet: PetState,
    pub group: GroupState,
    pub capabilities: CapabilityState,
    pub control: ControlState,
    pub desync: DesyncState,
    #[serde(default)]
    pub active_casts: BTreeMap<EntityId, ActiveCastState>,
}
