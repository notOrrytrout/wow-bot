use crate::workers::lifecycle::WorkerFailure;
use std::time::{Duration, Instant};
#[derive(Clone, Debug)]
pub struct Heartbeat {
    pub last: Instant,
    pub timeout: Duration,
}
impl Heartbeat {
    pub fn healthy(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last) <= self.timeout
    }
    pub fn classify(&self, now: Instant) -> Option<WorkerFailure> {
        (!self.healthy(now)).then_some(WorkerFailure::LoginTimeout)
    }
}
