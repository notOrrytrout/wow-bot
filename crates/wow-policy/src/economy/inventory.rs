use wow_state::Snapshot;
pub fn has_item(state: &Snapshot, item: u32, count: u32) -> bool {
    state.state.inventory.has(item, count)
}
pub fn has_space(state: &Snapshot) -> bool {
    state.state.inventory.free_slots > 0
}
