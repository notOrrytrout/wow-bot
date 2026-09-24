use serde::{Deserialize, Serialize};
use wow_domain::{LaneId, Mission, PauseReasons, WorkerGeneration};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SupervisorCommand {
    StartLane(LaneId),
    StopLane(LaneId),
    ReplaceMission {
        lane: LaneId,
        mission: Mission,
    },
    SetPause {
        lane: LaneId,
        reasons: PauseReasons,
    },
    UpdatePause {
        lane: LaneId,
        set: PauseReasons,
        clear: PauseReasons,
    },
    Shutdown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorkerEvent {
    Ready {
        lane: LaneId,
        generation: WorkerGeneration,
    },
    Heartbeat {
        lane: LaneId,
        generation: WorkerGeneration,
    },
    Failed {
        lane: LaneId,
        generation: WorkerGeneration,
        reason: String,
    },
    Stopped {
        lane: LaneId,
        generation: WorkerGeneration,
    },
}
