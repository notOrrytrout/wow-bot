use wow_state::Snapshot;
#[derive(Clone, Copy, Debug)]
pub struct SpellSemantics {
    pub spell: u32,
    pub max_range: f32,
    pub hostile: bool,
    pub breaks_cc: bool,
    pub interrupt: bool,
}
pub fn knows_spell(state: &Snapshot, spell: u32) -> bool {
    state.state.capabilities.spells.contains(&spell)
}
pub fn mechanically_legal(state: &Snapshot, sem: SpellSemantics, distance: f32) -> bool {
    knows_spell(state, sem.spell) && distance.is_finite() && distance <= sem.max_range
}
