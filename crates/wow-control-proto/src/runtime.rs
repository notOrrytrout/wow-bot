use crate::{ProxyToWorker, SupervisorCommand, WorkerEvent, WorkerToProxy};
use serde::{Deserialize, Serialize};
use wow_domain::{LaneId, WorkerGeneration};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorkerWire {
    Hello {
        lane: LaneId,
        generation: WorkerGeneration,
    },
    Proxy(WorkerToProxy),
    Event(WorkerEvent),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SupervisorWire {
    Proxy(ProxyToWorker),
    Command(SupervisorCommand),
}
