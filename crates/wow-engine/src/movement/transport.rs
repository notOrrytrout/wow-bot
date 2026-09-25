use wow_domain::{EntityId, WorldPosition};
use wow_state::transport::TransportState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportPhase {
    ApproachBoardingPoint,
    AwaitBoarding,
    Riding,
    Exit,
    Complete,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportAction {
    MoveToBoardingPoint,
    WaitForBoarding,
    WaitForRide,
    MoveToExitPoint,
    Complete,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportTraversal {
    pub transport: EntityId,
    /// Caller-supplied grounded point already accepted by normal navigation.
    pub boarding_point: WorldPosition,
    /// Caller-supplied grounded point already accepted by normal navigation.
    pub exit_point: WorldPosition,
    pub proximity: f32,
    pub max_vertical_delta: f32,
    pub max_observations: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportProgress {
    pub phase: TransportPhase,
    pub observations: u8,
}

impl Default for TransportProgress {
    fn default() -> Self {
        Self {
            phase: TransportPhase::ApproachBoardingPoint,
            observations: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportStep {
    pub progress: TransportProgress,
    pub action: TransportAction,
}

/// Advance one bounded step using only authoritative passenger attachment and
/// caller-supplied, grounded waypoints. This helper never creates a waypoint.
pub fn advance_transport(
    traversal: TransportTraversal,
    mut progress: TransportProgress,
    current: Option<WorldPosition>,
    observed: &TransportState,
) -> TransportStep {
    if progress.phase == TransportPhase::Complete {
        return step(progress, TransportAction::Complete);
    }
    if progress.phase == TransportPhase::Blocked {
        return step(progress, TransportAction::Blocked);
    }
    if traversal.max_observations == 0 || progress.observations >= traversal.max_observations {
        progress.phase = TransportPhase::Blocked;
        return step(progress, TransportAction::Blocked);
    }
    progress.observations = progress.observations.saturating_add(1);

    match progress.phase {
        TransportPhase::ApproachBoardingPoint => {
            if !safe_local_waypoint(
                current,
                traversal.boarding_point,
                traversal.max_vertical_delta,
            ) {
                progress.phase = TransportPhase::Blocked;
                return step(progress, TransportAction::Blocked);
            }
            if near(current, traversal.boarding_point, traversal.proximity) {
                progress.phase = TransportPhase::AwaitBoarding;
                step(progress, TransportAction::WaitForBoarding)
            } else {
                step(progress, TransportAction::MoveToBoardingPoint)
            }
        }
        TransportPhase::AwaitBoarding => {
            if observed.attached == Some(true) && observed.transport != Some(traversal.transport) {
                progress.phase = TransportPhase::Blocked;
                return step(progress, TransportAction::Blocked);
            }
            if observed.has_authoritative_attachment(traversal.transport) {
                progress.phase = TransportPhase::Riding;
                step(progress, TransportAction::WaitForRide)
            } else {
                step(progress, TransportAction::WaitForBoarding)
            }
        }
        TransportPhase::Riding => {
            if observed.attached == Some(false) {
                if !safe_local_waypoint(current, traversal.exit_point, traversal.max_vertical_delta)
                {
                    progress.phase = TransportPhase::Blocked;
                    return step(progress, TransportAction::Blocked);
                }
                if near(current, traversal.exit_point, traversal.proximity) {
                    progress.phase = TransportPhase::Complete;
                    step(progress, TransportAction::Complete)
                } else {
                    progress.phase = TransportPhase::Exit;
                    step(progress, TransportAction::MoveToExitPoint)
                }
            } else if observed.has_authoritative_attachment(traversal.transport) {
                step(progress, TransportAction::WaitForRide)
            } else {
                progress.phase = TransportPhase::Blocked;
                step(progress, TransportAction::Blocked)
            }
        }
        TransportPhase::Exit => {
            if !safe_local_waypoint(current, traversal.exit_point, traversal.max_vertical_delta) {
                progress.phase = TransportPhase::Blocked;
                return step(progress, TransportAction::Blocked);
            }
            if near(current, traversal.exit_point, traversal.proximity) {
                progress.phase = TransportPhase::Complete;
                step(progress, TransportAction::Complete)
            } else {
                step(progress, TransportAction::MoveToExitPoint)
            }
        }
        TransportPhase::Complete => step(progress, TransportAction::Complete),
        TransportPhase::Blocked => step(progress, TransportAction::Blocked),
    }
}

fn safe_local_waypoint(
    current: Option<WorldPosition>,
    waypoint: WorldPosition,
    max_vertical_delta: f32,
) -> bool {
    current.is_some_and(|position| {
        position.point.is_finite()
            && waypoint.point.is_finite()
            && position.map == waypoint.map
            && max_vertical_delta.is_finite()
            && max_vertical_delta >= 0.0
            && (position.point.z - waypoint.point.z).abs() <= max_vertical_delta
    })
}

fn near(current: Option<WorldPosition>, waypoint: WorldPosition, radius: f32) -> bool {
    current.is_some_and(|position| {
        radius.is_finite() && radius >= 0.0 && position.point.distance(waypoint.point) <= radius
    })
}

fn step(progress: TransportProgress, action: TransportAction) -> TransportStep {
    TransportStep { progress, action }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::Vec3;

    fn pos(x: f32, z: f32) -> WorldPosition {
        WorldPosition {
            map: 1,
            point: Vec3::new(x, 0.0, z),
            orientation: 0.0,
        }
    }

    fn traversal(max_observations: u8) -> TransportTraversal {
        TransportTraversal {
            transport: EntityId(7),
            boarding_point: pos(0.0, 0.0),
            exit_point: pos(10.0, 0.0),
            proximity: 1.0,
            max_vertical_delta: 2.0,
            max_observations,
        }
    }

    #[test]
    fn waits_for_authoritative_boarding_and_exit() {
        let spec = traversal(8);
        let first = advance_transport(
            spec,
            TransportProgress::default(),
            Some(pos(3.0, 0.0)),
            &TransportState::default(),
        );
        assert_eq!(first.action, TransportAction::MoveToBoardingPoint);

        let board = advance_transport(
            spec,
            first.progress,
            Some(pos(0.0, 0.0)),
            &TransportState::default(),
        );
        assert_eq!(board.action, TransportAction::WaitForBoarding);

        let on_transport = TransportState {
            attached: Some(true),
            transport: Some(EntityId(7)),
            relative_position: Some(Vec3::default()),
            relative_orientation: Some(0.0),
            transport_time: Some(5),
        };
        let ride = advance_transport(spec, board.progress, Some(pos(0.0, 0.0)), &on_transport);
        assert_eq!(ride.action, TransportAction::WaitForRide);

        let off_transport = TransportState {
            attached: Some(false),
            ..Default::default()
        };
        let exit = advance_transport(spec, ride.progress, Some(pos(0.0, 0.0)), &off_transport);
        assert_eq!(exit.action, TransportAction::MoveToExitPoint);
        let done = advance_transport(spec, exit.progress, Some(pos(10.0, 0.0)), &off_transport);
        assert_eq!(done.action, TransportAction::Complete);
    }

    #[test]
    fn rejects_cross_floor_waypoints_and_missing_relative_attachment() {
        let mut spec = traversal(3);
        spec.boarding_point = pos(0.0, 20.0);
        let blocked = advance_transport(
            spec,
            TransportProgress::default(),
            Some(pos(0.0, 0.0)),
            &TransportState::default(),
        );
        assert_eq!(blocked.action, TransportAction::Blocked);

        let mut spec = traversal(3);
        let no_relative_pose = TransportState {
            attached: Some(true),
            transport: Some(EntityId(7)),
            ..Default::default()
        };
        let waiting = advance_transport(
            spec,
            TransportProgress {
                phase: TransportPhase::AwaitBoarding,
                observations: 0,
            },
            Some(pos(0.0, 0.0)),
            &no_relative_pose,
        );
        assert_eq!(waiting.action, TransportAction::WaitForBoarding);
        spec.max_observations = 1;
        let blocked = advance_transport(
            spec,
            waiting.progress,
            Some(pos(0.0, 0.0)),
            &no_relative_pose,
        );
        assert_eq!(blocked.action, TransportAction::Blocked);
    }

    #[test]
    fn bounds_observation_retries() {
        let spec = traversal(1);
        let first = advance_transport(
            spec,
            TransportProgress::default(),
            Some(pos(3.0, 0.0)),
            &TransportState::default(),
        );
        let second = advance_transport(
            spec,
            first.progress,
            Some(pos(3.0, 0.0)),
            &TransportState::default(),
        );
        assert_eq!(second.action, TransportAction::Blocked);
    }
}
