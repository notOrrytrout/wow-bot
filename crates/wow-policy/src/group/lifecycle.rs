#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupWorkState {
    Forming,
    Following,
    Engaging,
    Recovering,
    Regrouping,
}
pub fn after_encounter(formation_broken: bool) -> GroupWorkState {
    if formation_broken {
        GroupWorkState::Regrouping
    } else {
        GroupWorkState::Following
    }
}
