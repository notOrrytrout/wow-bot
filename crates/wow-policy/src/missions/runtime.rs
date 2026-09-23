use wow_domain::{Mission, MissionRevision, TaskId};

#[derive(Clone, Debug)]
pub struct MissionRuntime {
    pub mission: Mission,
    pub revision: MissionRevision,
    pub active_task: Option<TaskId>,
    pub failed_attempts: u8,
}

impl MissionRuntime {
    pub fn new(mission: Mission, revision: MissionRevision) -> Self { Self { mission, revision, active_task: None, failed_attempts: 0 } }
    pub fn reset(&mut self, mission: Mission) {
        self.mission = mission;
        self.revision = self.revision.next();
        self.active_task = None;
        self.failed_attempts = 0;
    }
    pub fn note_failure(&mut self, max_attempts: u8) -> bool {
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        if self.failed_attempts >= max_attempts { self.active_task = None; true } else { false }
    }
}
