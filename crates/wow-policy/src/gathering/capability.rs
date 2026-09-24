use wow_state::Snapshot;

pub fn has_required_skill(state: &Snapshot, skill: u32, required: u16) -> bool {
    state.state.professions.skill(skill) >= required
}
pub fn can_fish(state: &Snapshot) -> bool {
    state.state.professions.fishing && state.state.capabilities.can_fish
}
