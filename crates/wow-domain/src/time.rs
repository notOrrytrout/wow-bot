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
}
