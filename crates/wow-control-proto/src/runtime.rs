use serde::{Deserialize, Serialize};
use wow_domain::{LaneId, WorkerGeneration};
use crate::{ProxyToWorker, SupervisorCommand, WorkerEvent, WorkerToProxy};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorkerWire {
    Hello { lane: LaneId, generation: WorkerGeneration },
    Proxy(WorkerToProxy),
    Event(WorkerEvent),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SupervisorWire {
    Proxy(ProxyToWorker),
    Command(SupervisorCommand),
}
