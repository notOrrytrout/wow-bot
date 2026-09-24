use wow_domain::{MovementEpoch, MovementId, Vec3};
#[derive(Clone, Debug)]
pub struct MovementRequest {
    pub id: MovementId,
    pub epoch: MovementEpoch,
    pub destination: Vec3,
}
#[derive(Clone, Copy, Debug)]
pub struct MovementClock {
    last: u32,
}
impl MovementClock {
    pub fn new(observed: u32) -> Self {
        Self { last: observed }
    }
    pub fn observe(&mut self, t: u32) {
        if t.wrapping_sub(self.last) < 0x8000_0000 {
            self.last = t
        }
    }
    pub fn next(&mut self, elapsed_ms: u32) -> u32 {
        let increment = elapsed_ms.max(1);
        self.last = self.last.wrapping_add(increment);
        self.last
    }
    pub fn last(self) -> u32 {
        self.last
    }
}
