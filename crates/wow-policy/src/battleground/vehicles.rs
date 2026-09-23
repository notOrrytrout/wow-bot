use wow_domain::EntityId;use std::collections::BTreeSet;
#[derive(Clone,Debug,Default)]pub struct VehicleState{pub vehicle:Option<EntityId>,pub observed_abilities:BTreeSet<u32>}
pub fn can_enter(observed:Option<EntityId>,expected:EntityId)->bool{observed==Some(expected)}
pub fn can_use(state:&VehicleState,spell:u32)->bool{state.vehicle.is_some()&&state.observed_abilities.contains(&spell)}
