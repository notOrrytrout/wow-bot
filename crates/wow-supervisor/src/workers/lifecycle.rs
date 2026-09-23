use wow_domain::{LaneId, WorkerGeneration};

#[derive(Clone, Copy, Debug)]
pub struct RestartPolicy { pub max_restarts: u32, pub base_backoff_ms: u64 }
impl Default for RestartPolicy { fn default() -> Self { Self { max_restarts: 10, base_backoff_ms: 500 } } }

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerFailure {
    ExpectedTakeover,
    Cancelled,
    Protocol(String),
    Io(String),
    StartupTimeout,
    LoginTimeout,
    ProcessExit(Option<i32>),
}

#[derive(Clone, Debug)]
pub struct WorkerLease { pub lane: LaneId, pub generation: WorkerGeneration }
impl WorkerLease {
    pub fn is_current(&self, lane: LaneId, generation: WorkerGeneration) -> bool { self.lane == lane && self.generation == generation }
}
