use wow_domain::PauseReasons;
pub fn merge(current: &mut PauseReasons, set: PauseReasons, clear: PauseReasons) {
    current.remove(clear);
    current.insert(set)
}
