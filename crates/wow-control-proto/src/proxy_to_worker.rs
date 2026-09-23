use serde::{Deserialize,Serialize};
use wow_domain::{ActionId,MovementEpoch,OwnershipGeneration,WorkerGeneration};
use wow_state::ProtocolObservation;
#[derive(Clone,Debug,Serialize,Deserialize)] pub enum ActionTransportResult{Accepted,Rejected{reason:String}}
#[derive(Clone,Debug,Serialize,Deserialize)] pub struct OwnershipSnapshot{pub generation:OwnershipGeneration,pub movement_epoch:MovementEpoch,pub player_present:bool,pub bot_allowed:bool}
#[derive(Clone,Debug,Serialize,Deserialize)] pub enum ProxyToWorker{Observation(ProtocolObservation),OwnershipChanged(OwnershipSnapshot),MovementFence(MovementEpoch),ActionResult{action:ActionId,result:ActionTransportResult},SessionState{connected:bool,in_world:bool},WorkerGeneration(WorkerGeneration),Shutdown}
