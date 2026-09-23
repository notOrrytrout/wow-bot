use tokio::sync::{mpsc, watch};
use wow_control_proto::SupervisorCommand;
use wow_domain::{LaneId, PauseReasons};

#[derive(Clone)]
pub struct ControlService { tx: mpsc::Sender<SupervisorCommand> }
impl ControlService {
    pub fn new(tx:mpsc::Sender<SupervisorCommand>)->Self{Self{tx}}
    pub async fn submit(&self,c:SupervisorCommand)->Result<(),String>{self.tx.send(c).await.map_err(|_|"control channel closed".into())}
    pub fn try_submit(&self,c:SupervisorCommand)->Result<(),String>{self.tx.try_send(c).map_err(|e|format!("worker command queue unavailable: {e}"))}
}

pub struct LaneControlState {
    pub pause_tx: watch::Sender<PauseReasons>,
    pub lane: LaneId,
}
impl LaneControlState {
    pub fn set_pause(&self, reasons: PauseReasons) { self.pause_tx.send_replace(reasons); }
}
