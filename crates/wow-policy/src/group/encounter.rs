use wow_domain::EntityId;use wow_state::Snapshot;
#[derive(Clone,Debug,Default)]pub struct EncounterIntent{pub preferred_target:Option<EntityId>,pub allowed_targets:Vec<EntityId>,pub interrupt_allowed:bool,pub hold_threat:bool}
pub fn from_observed(state:&Snapshot)->EncounterIntent{let target=state.state.group.encounter_target;EncounterIntent{preferred_target:target,allowed_targets:target.into_iter().collect(),interrupt_allowed:target.is_some(),hold_threat:false}}
