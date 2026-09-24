use crate::{configured_session::player::PlayerPresence, ownership::AccountOwnership};
use std::time::Duration;
use wow_domain::{AccountId, LaneId, WorkerGeneration};
pub struct ConfiguredSessionState {
    pub account: AccountId,
    pub lane: LaneId,
    pub worker: WorkerGeneration,
    pub ownership: AccountOwnership,
    pub player: PlayerPresence,
    pub upstream_connected: bool,
    pub world_authoritative: bool,
    pub worker_running: bool,
}
impl ConfiguredSessionState {
    pub fn new(account: AccountId, lane: LaneId, worker: WorkerGeneration) -> Self {
        Self {
            account,
            lane,
            worker,
            ownership: AccountOwnership::default(),
            player: PlayerPresence::new(Duration::from_secs(2)),
            upstream_connected: false,
            world_authoritative: false,
            worker_running: true,
        }
    }
}
