use wow_state::Snapshot;
pub fn learned(state: &Snapshot, skill: u32) -> bool { state.state.professions.skills.contains_key(&skill) }
