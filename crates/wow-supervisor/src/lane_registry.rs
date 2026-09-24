use std::collections::BTreeMap;
use wow_domain::{LaneId, PauseReasons, WorkerGeneration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaneStatus {
    Stopped,
    Starting,
    Ready,
    Failed,
    Stopping,
}
#[derive(Clone, Debug)]
pub struct LaneRecord {
    pub generation: WorkerGeneration,
    pub status: LaneStatus,
    pub restarts: u32,
    pub pause: PauseReasons,
}
#[derive(Default)]
pub struct LaneRegistry {
    lanes: BTreeMap<LaneId, LaneRecord>,
}
impl LaneRegistry {
    pub fn get(&self, l: LaneId) -> Option<&LaneRecord> {
        self.lanes.get(&l)
    }
    pub fn get_mut(&mut self, l: LaneId) -> Option<&mut LaneRecord> {
        self.lanes.get_mut(&l)
    }
    pub fn set(&mut self, l: LaneId, r: LaneRecord) {
        self.lanes.insert(l, r);
    }
    pub fn next_generation(&self, l: LaneId) -> WorkerGeneration {
        self.get(l)
            .map_or(WorkerGeneration(1), |r| r.generation.next())
    }
    pub fn update_if_current<F>(&mut self, l: LaneId, g: WorkerGeneration, f: F) -> bool
    where
        F: FnOnce(&mut LaneRecord),
    {
        let Some(r) = self.lanes.get_mut(&l) else {
            return false;
        };
        if r.generation != g {
            return false;
        }
        f(r);
        true
    }
}
