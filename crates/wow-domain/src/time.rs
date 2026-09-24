use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Millis(pub u64);

impl Millis {
    pub fn wall_clock_now() -> Self {
        let value = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        Self(value.as_millis().min(u128::from(u64::MAX)) as u64)
    }

    pub const fn saturating_add(self, duration_ms: u64) -> Self {
        Self(self.0.saturating_add(duration_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::Millis;

    #[test]
    fn saturating_add_caps_at_millisecond_limit() {
        assert_eq!(Millis(12).saturating_add(30), Millis(42));
        assert_eq!(Millis(u64::MAX - 1).saturating_add(2), Millis(u64::MAX));
    }
}
