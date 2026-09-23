use serde::{Deserialize, Serialize};
use wow_domain::EntityId;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GroupMember { pub entity: EntityId, pub name: String, pub role: Option<String>, pub online: bool }

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum GroupLifecycle { #[default] Solo, Forming, Active, Leaving }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GroupState {
    pub generation: u64,
    pub members: Vec<GroupMember>,
    pub leader: Option<EntityId>,
    pub raid: bool,
    pub lifecycle: GroupLifecycle,
    pub encounter_target: Option<EntityId>,
}
