use wow_domain::{ActivationStage, Mission, MovementEpoch, OwnershipGeneration, PauseReasons, ProposedAction};
use wow_state::ProtocolObservation;
pub enum LaneMessage {
    Observation(ProtocolObservation),
    Propose(ProposedAction),
    ReplaceMission(Mission),
    SetPause(PauseReasons),
    UpdatePause { set: PauseReasons, clear: PauseReasons },
    SetActivation(ActivationStage),
    Ownership { generation: OwnershipGeneration, movement: MovementEpoch, bot_allowed: bool },
    MovementFence(MovementEpoch),
    Shutdown,
}
