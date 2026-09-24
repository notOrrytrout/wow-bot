use wow_domain::EntityId;
use wow_state::Snapshot;
pub fn observed_member(state: &Snapshot, id: EntityId) -> bool {
    state
        .state
        .group
        .members
        .iter()
        .any(|m| m.entity == id && m.online)
}
pub fn leader(state: &Snapshot) -> Option<EntityId> {
    state.state.group.leader
}
