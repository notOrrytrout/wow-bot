use wow_domain::{MovementEpoch, MovementId};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplanToken {
    pub movement: MovementId,
    pub epoch: MovementEpoch,
}
