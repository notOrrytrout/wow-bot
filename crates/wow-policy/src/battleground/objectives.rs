use wow_domain::{EntityId,Vec3};
#[derive(Clone,Debug)]pub struct Objective{pub id:u32,pub observed_entity:Option<EntityId>,pub friendly:bool,pub trusted_position:Option<Vec3>}
pub fn can_interact(in_battleground:bool,objective:&Objective)->bool{in_battleground&&!objective.friendly&&objective.observed_entity.is_some()}
pub fn deterministic_position(objective:&Objective)->Option<Vec3>{objective.trusted_position}
