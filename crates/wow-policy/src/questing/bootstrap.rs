#[derive(Clone, Copy, Debug)]
pub struct BootstrapPolicy {
    pub empty_journal_radius: f32,
    pub active_radius: f32,
}
impl Default for BootstrapPolicy {
    fn default() -> Self {
        Self {
            empty_journal_radius: 1500.0,
            active_radius: 300.0,
        }
    }
}
impl BootstrapPolicy {
    pub fn radius(self, has_active: bool) -> f32 {
        if has_active {
            self.active_radius
        } else {
            self.empty_journal_radius
        }
    }
}
