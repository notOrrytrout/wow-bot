use crate::activity::ActivityArbiter;
use wow_domain::*;
use wow_state::AuthoritativeState;
pub struct LaneState {
    pub lane: LaneId,
    pub worker: WorkerGeneration,
    pub ownership: OwnershipGeneration,
    pub movement_epoch: MovementEpoch,
    pub mission: Mission,
    pub mission_revision: MissionRevision,
    pub permission_revision: PermissionRevision,
    pub pause: PauseReasons,
    pub activation: ActivationStage,
    pub authoritative: AuthoritativeState,
    pub activity: ActivityArbiter,
}
impl LaneState {
    pub fn stamp(&self) -> ValidityStamp {
        ValidityStamp {
            state: self.authoritative.revision,
            mission: self.mission_revision,
            permission: self.permission_revision,
            worker: self.worker,
            ownership: self.ownership,
            movement: self.movement_epoch,
        }
    }
    pub fn runnable(&self) -> bool {
        self.pause.is_empty()
    }
}
