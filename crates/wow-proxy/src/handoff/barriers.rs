#[derive(Clone,Copy,Debug,Default)]pub struct HandoffBarrier{pub worker_quiesced:bool,pub ownership_committed:bool,pub movement_stopped:bool,pub upstream_ready:bool}
impl HandoffBarrier{pub fn takeover_ready(self)->bool{self.worker_quiesced&&self.ownership_committed}pub fn resume_ready(self)->bool{self.worker_quiesced&&self.ownership_committed&&self.movement_stopped&&self.upstream_ready}}
