use std::f32::consts::{PI, TAU};

const MELEE_FACING_TOLERANCE: f32 = PI / 6.0;
const INTERACTION_FACING_TOLERANCE: f32 = PI / 4.0;
const CAST_FACING_TOLERANCE: f32 = PI / 9.0;

use wow_domain::{EntityId, FacingRequirement, GameplayCommand, LineOfSightRequirement, MovementRequirement, Vec3, WorldPosition};
use wow_state::Snapshot;

/// Shared spatial contract for a targeted action. Mission code selects the
/// semantic action; this module owns reusable positioning prerequisites.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialProfile {
    pub target: EntityId,
    pub maximum_range: f32,
    pub approach_range: f32,
    pub facing_tolerance: Option<f32>,
    pub requires_line_of_sight: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineOfSightStatus { Unknown, Clear, Blocked }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerRangeCorrection { MoveCloser, MoveFarther }

pub fn profile(command: &GameplayCommand) -> Option<SpatialProfile> {
    let (target, maximum_range, approach_range, facing_tolerance, requires_line_of_sight) = match command {
        GameplayCommand::Attack(target) => (*target, 5.0, 4.0, Some(MELEE_FACING_TOLERANCE), true),
        GameplayCommand::Interact(target)
        | GameplayCommand::UseGameObject(target)
        | GameplayCommand::CastGameObject { target, .. }
        | GameplayCommand::Loot(target)
        | GameplayCommand::Gather(target)
        | GameplayCommand::EnterVehicle(target) => (*target, 5.0, 4.0, Some(INTERACTION_FACING_TOLERANCE), true),
        GameplayCommand::UseItem { target: Some(target), .. }
        | GameplayCommand::UseItemInstance { target: Some(target), .. } => (*target, 5.0, 4.0, Some(INTERACTION_FACING_TOLERANCE), true),
        GameplayCommand::Cast { target: Some(target), .. }
        | GameplayCommand::MaintainBuff { target, .. }
        | GameplayCommand::VehicleCast { target: Some(target), .. } => (*target, 20.0, 18.0, Some(CAST_FACING_TOLERANCE), true),
        GameplayCommand::AcceptQuest { giver: target, .. }
        | GameplayCommand::TurnInQuest { giver: target, .. }
        | GameplayCommand::RequestQuestReward { giver: target, .. }
        | GameplayCommand::ChooseQuestReward { giver: target, .. }
        | GameplayCommand::VendorBuy { vendor: target, .. }
        | GameplayCommand::VendorSell { vendor: target, .. } => (*target, 5.0, 4.0, Some(INTERACTION_FACING_TOLERANCE), true),
        _ => return None,
    };
    Some(SpatialProfile { target, maximum_range, approach_range, facing_tolerance, requires_line_of_sight })
}

pub fn active_mover(snapshot: &Snapshot) -> Option<WorldPosition> {
    snapshot.state.control.active_position(snapshot.state.position.player)
}

pub fn target_position(snapshot: &Snapshot, target: EntityId) -> Option<WorldPosition> {
    snapshot.state.entities.0.get(&target)?.position
}

pub fn movement_requirement(snapshot: &Snapshot, command: &GameplayCommand) -> Option<MovementRequirement> {
    let profile = profile(command)?;
    let mover = active_mover(snapshot)?;
    let target = target_position(snapshot, profile.target)?;
    if mover.map != target.map { return None; }
    (mover.point.distance(target.point) > profile.maximum_range).then_some(MovementRequirement {
        destination: target.point,
        acceptable_range: profile.approach_range,
    })
}

pub fn facing_requirement(snapshot: &Snapshot, command: &GameplayCommand) -> Option<FacingRequirement> {
    let profile = profile(command)?;
    let tolerance = profile.facing_tolerance?;
    let mover = active_mover(snapshot)?;
    let target = target_position(snapshot, profile.target)?;
    if mover.map != target.map { return None; }
    let desired = desired_orientation(mover.point, target.point)?;
    (angular_distance(mover.orientation, desired) > tolerance).then_some(FacingRequirement {
        orientation: desired,
        tolerance,
    })
}

pub fn line_of_sight_requirement(
    command: &GameplayCommand,
    status: impl FnOnce(EntityId) -> LineOfSightStatus,
) -> Option<LineOfSightRequirement> {
    let profile = profile(command)?;
    if !profile.requires_line_of_sight { return None; }
    (status(profile.target) == LineOfSightStatus::Blocked).then_some(LineOfSightRequirement { target: profile.target })
}

pub fn desired_orientation(from: Vec3, to: Vec3) -> Option<f32> {
    if !from.is_finite() || !to.is_finite() { return None; }
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    if dx.abs() < f32::EPSILON && dy.abs() < f32::EPSILON { return None; }
    Some(dy.atan2(dx).rem_euclid(TAU))
}

pub fn angular_distance(a: f32, b: f32) -> f32 {
    if !a.is_finite() || !b.is_finite() { return f32::INFINITY; }
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d)
}

pub fn server_range_reposition(
    mover: WorldPosition,
    target: WorldPosition,
    correction: ServerRangeCorrection,
    attempt: u8,
) -> Option<Vec3> {
    if mover.map != target.map || !mover.point.is_finite() || !target.point.is_finite() { return None; }
    let dx = target.point.x - mover.point.x;
    let dy = target.point.y - mover.point.y;
    let len = dx.hypot(dy);
    if len < 0.001 { return None; }
    let ux = dx / len;
    let uy = dy / len;
    let step = 2.5 + f32::from(attempt.min(3));
    let sign = match correction { ServerRangeCorrection::MoveCloser => 1.0, ServerRangeCorrection::MoveFarther => -1.0 };
    Some(Vec3::new(mover.point.x + ux * step * sign, mover.point.y + uy * step * sign, mover.point.z))
}

/// Deterministic lateral candidate used after an authoritative server LOS
/// failure. The caller still routes this point through the shared movement
/// controller, so terrain/flying rules remain centralized.
pub fn line_of_sight_reposition(mover: WorldPosition, target: WorldPosition, attempt: u8) -> Option<Vec3> {
    if mover.map != target.map || !mover.point.is_finite() || !target.point.is_finite() { return None; }
    let dx = target.point.x - mover.point.x;
    let dy = target.point.y - mover.point.y;
    let len = dx.hypot(dy);
    if len < 0.001 { return None; }
    let px = -dy / len;
    let py = dx / len;
    let side = if attempt % 2 == 0 { 1.0 } else { -1.0 };
    let radius = 3.0 + f32::from(attempt.min(3));
    Some(Vec3::new(mover.point.x + px * radius * side, mover.point.y + py * radius * side, mover.point.z))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{EntityId, GameplayCommand, Vec3, WorldPosition};
    use wow_state::{entities::{EntityKind, EntityState}, AuthoritativeState, Snapshot};

    fn snapshot(mover: WorldPosition, target: WorldPosition) -> Snapshot {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(mover);
        state.entities.0.insert(EntityId(7), EntityState { id: EntityId(7), entry: 1, kind: EntityKind::Unit, name: None, position: Some(target), health: None, hostile: true, interactable: true, ..Default::default() });
        Snapshot::from_state(&state)
    }

    #[test]
    fn range_and_facing_are_shared_for_targeted_actions() {
        let s = snapshot(
            WorldPosition { map: 1, point: Vec3::new(0.0, 0.0, 0.0), orientation: PI },
            WorldPosition { map: 1, point: Vec3::new(10.0, 0.0, 0.0), orientation: 0.0 },
        );
        let command = GameplayCommand::Attack(EntityId(7));
        assert!(movement_requirement(&s, &command).is_some());
        assert!(facing_requirement(&s, &command).is_some());
    }

    #[test]
    fn interaction_approach_keeps_margin_inside_server_range() {
        let s = snapshot(
            WorldPosition { map: 1, point: Vec3::new(0.0, 0.0, 0.0), orientation: 0.0 },
            WorldPosition { map: 1, point: Vec3::new(5.1, 0.0, 0.6), orientation: 0.0 },
        );
        let requirement = movement_requirement(&s, &GameplayCommand::Loot(EntityId(7))).unwrap();
        assert_eq!(requirement.acceptable_range, 4.0);
    }

    #[test]
    fn authoritative_range_recovery_moves_in_the_requested_direction() {
        let mover = WorldPosition { map: 1, point: Vec3::new(0.0, 0.0, 5.0), orientation: 0.0 };
        let target = WorldPosition { map: 1, point: Vec3::new(10.0, 0.0, 5.0), orientation: 0.0 };
        let closer = server_range_reposition(mover, target, ServerRangeCorrection::MoveCloser, 0).unwrap();
        let farther = server_range_reposition(mover, target, ServerRangeCorrection::MoveFarther, 0).unwrap();
        assert!(closer.x > mover.point.x);
        assert!(farther.x < mover.point.x);
    }

    #[test]
    fn los_reposition_is_deterministic_and_alternates_side() {
        let mover = WorldPosition { map: 1, point: Vec3::new(0.0, 0.0, 5.0), orientation: 0.0 };
        let target = WorldPosition { map: 1, point: Vec3::new(10.0, 0.0, 5.0), orientation: 0.0 };
        let a = line_of_sight_reposition(mover, target, 0).unwrap();
        let b = line_of_sight_reposition(mover, target, 1).unwrap();
        assert!(a.y > 0.0 && b.y < 0.0);
        assert_eq!(a.z, mover.point.z);
    }
}
