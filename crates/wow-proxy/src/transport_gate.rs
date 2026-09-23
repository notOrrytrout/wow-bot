use crate::ownership::{ClientKind, OwnershipSnapshot};
use wow_domain::{GameplayCommand, SendableAction, WorkerGeneration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateRejection { StaleWorker, StaleOwnership, StaleMovement, SourceNotPermitted }

pub fn authorize(action: &SendableAction, current_worker: WorkerGeneration, owner: OwnershipSnapshot) -> Result<(), GateRejection> {
    let stamp = action.stamp();
    if stamp.worker != current_worker { return Err(GateRejection::StaleWorker); }
    if stamp.ownership != owner.generation { return Err(GateRejection::StaleOwnership); }
    if !owner.permits(ClientKind::Bot) { return Err(GateRejection::SourceNotPermitted); }
    if matches!(action.command(), GameplayCommand::MoveTo(_) | GameplayCommand::FaceDirection { .. } | GameplayCommand::StopMovement) && stamp.movement != owner.movement_epoch {
        return Err(GateRejection::StaleMovement);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ownership::{AccountOwnership, ControlMode};
    use wow_domain::{ActionId, MovementEpoch, OwnershipGeneration, PlanOrigin, TaskId, ValidatedAction, ValidityStamp, Vec3};

    fn action(stamp: ValidityStamp, command: GameplayCommand) -> SendableAction {
        SendableAction::from_validated(ValidatedAction {
            id: ActionId(1),
            task: TaskId(1),
            origin: PlanOrigin::Operator,
            stamp,
            command,
        })
    }

    #[test]
    fn stale_ownership_is_rejected_at_transport_edge() {
        let owner = AccountOwnership::new(ControlMode::Bot).snapshot();
        let stamp = ValidityStamp {
            worker: WorkerGeneration(1),
            ownership: OwnershipGeneration(owner.generation.0.wrapping_add(1)),
            movement: owner.movement_epoch,
            ..ValidityStamp::default()
        };
        assert_eq!(authorize(&action(stamp, GameplayCommand::StopMovement), WorkerGeneration(1), owner), Err(GateRejection::StaleOwnership));
    }

    #[test]
    fn stale_movement_epoch_is_rejected_even_when_ownership_matches() {
        let owner = AccountOwnership::new(ControlMode::Bot).snapshot();
        let stamp = ValidityStamp {
            worker: WorkerGeneration(1),
            ownership: owner.generation,
            movement: MovementEpoch(owner.movement_epoch.0.wrapping_add(1)),
            ..ValidityStamp::default()
        };
        assert_eq!(authorize(&action(stamp, GameplayCommand::MoveTo(Vec3 { x: 1.0, y: 2.0, z: 3.0 })), WorkerGeneration(1), owner), Err(GateRejection::StaleMovement));
    }
}
