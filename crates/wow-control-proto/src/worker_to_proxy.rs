use serde::{Deserialize, Serialize};
use wow_domain::{MovementEpoch, MovementId, SendableAction, Vec3};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorkerToProxy {
    Ready,
    Action(SendableAction),
    Movement {
        movement: MovementId,
        epoch: MovementEpoch,
        destination: Vec3,
    },
    StopMovement {
        movement: MovementId,
        epoch: MovementEpoch,
    },
    QuerySession,
    ShutdownAck,
}
