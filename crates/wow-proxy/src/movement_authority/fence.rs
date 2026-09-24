use wow_domain::MovementEpoch;
#[derive(Clone, Copy, Debug)]
pub struct MovementFence {
    epoch: MovementEpoch,
}
impl MovementFence {
    pub fn new(epoch: MovementEpoch) -> Self {
        Self { epoch }
    }
    pub fn current(&self) -> MovementEpoch {
        self.epoch
    }
    pub fn advance(&mut self) -> MovementEpoch {
        self.epoch = self.epoch.next();
        self.epoch
    }
    pub fn accepts(&self, e: MovementEpoch) -> bool {
        e == self.epoch
    }
}
