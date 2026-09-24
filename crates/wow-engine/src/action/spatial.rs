use std::f32::consts::{PI, TAU};

const MELEE_FACING_TOLERANCE: f32 = PI / 6.0;
const INTERACTION_FACING_TOLERANCE: f32 = PI / 4.0;
const CAST_FACING_TOLERANCE: f32 = PI / 9.0;
const NPC_FRONT_STANDOFF: f32 = 3.0;
const NPC_FRONT_TOLERANCE: f32 = 1.5;
const MOB_REAR_STANDOFF: f32 = 3.0;
const MOB_REAR_TOLERANCE: f32 = 1.0;
const MOB_MELEE_MAX_RANGE: f32 = 5.0;
const MOB_MELEE_APPROACH_RANGE: f32 = 4.0;
const MOB_CAST_MAX_RANGE: f32 = 20.0;
const MOB_CAST_APPROACH_RANGE: f32 = 18.0;

use wow_domain::{
    EntityId, FacingRequirement, GameplayCommand, LineOfSightRequirement, MovementRequirement,
    Vec3, WorldPosition,
};
use wow_state::Snapshot;

/// Shared spatial contract for a targeted action. Mission code selects the
/// semantic action; this module owns reusable positioning prerequisites.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialProfile {
    pub target: EntityId,
    pub minimum_range: f32,
    pub maximum_range: f32,
    pub approach_range: f32,
    pub facing_tolerance: Option<f32>,
    pub requires_line_of_sight: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineOfSightStatus {
    Unknown,
    Clear,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerRangeCorrection {
    MoveCloser,
    MoveFarther,
}

pub fn profile(command: &GameplayCommand) -> Option<SpatialProfile> {
    let (
        target,
        minimum_range,
        maximum_range,
        approach_range,
        facing_tolerance,
        requires_line_of_sight,
    ) = match command {
        GameplayCommand::Attack(target) => (
            *target,
            0.0,
            MOB_MELEE_MAX_RANGE,
            MOB_MELEE_APPROACH_RANGE,
            Some(MELEE_FACING_TOLERANCE),
            true,
        ),
        GameplayCommand::Interact(target)
        | GameplayCommand::UseGameObject(target)
        | GameplayCommand::CastGameObject { target, .. }
        | GameplayCommand::Loot(target)
        | GameplayCommand::Gather(target)
        | GameplayCommand::EnterVehicle(target) => (
            *target,
            0.0,
            5.0,
            4.0,
            Some(INTERACTION_FACING_TOLERANCE),
            true,
        ),
        GameplayCommand::UseItem {
            target: Some(target),
            ..
        }
        | GameplayCommand::UseItemInstance {
            target: Some(target),
            ..
        } => (
            *target,
            0.0,
            5.0,
            4.0,
            Some(INTERACTION_FACING_TOLERANCE),
            true,
        ),
        GameplayCommand::Cast {
            target: Some(target),
            ..
        } => (
            *target,
            0.0,
            MOB_CAST_MAX_RANGE,
            MOB_CAST_APPROACH_RANGE,
            Some(CAST_FACING_TOLERANCE),
            true,
        ),
        GameplayCommand::MaintainBuff { target, .. }
        | GameplayCommand::VehicleCast {
            target: Some(target),
            ..
        } => (
            *target,
            0.0,
            MOB_CAST_MAX_RANGE,
            MOB_CAST_APPROACH_RANGE,
            Some(CAST_FACING_TOLERANCE),
            true,
        ),
        GameplayCommand::AcceptQuest { giver: target, .. }
        | GameplayCommand::TurnInQuest { giver: target, .. }
        | GameplayCommand::RequestQuestReward { giver: target, .. }
        | GameplayCommand::ChooseQuestReward { giver: target, .. }
        | GameplayCommand::VendorBuy { vendor: target, .. }
        | GameplayCommand::VendorSell { vendor: target, .. } => (
            *target,
            0.0,
            5.0,
            4.0,
            Some(INTERACTION_FACING_TOLERANCE),
            true,
        ),
        _ => return None,
    };
    Some(SpatialProfile {
        target,
        minimum_range,
        maximum_range,
        approach_range,
        facing_tolerance,
        requires_line_of_sight,
    })
}

pub fn active_mover(snapshot: &Snapshot) -> Option<WorldPosition> {
    snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)
}

pub fn target_position(snapshot: &Snapshot, target: EntityId) -> Option<WorldPosition> {
    snapshot.state.entities.0.get(&target)?.position
}

/// Return a movement requirement that places the bot within melee reach.
pub fn mob_melee_approach_requirement(
    mover: WorldPosition,
    target: WorldPosition,
) -> Option<MovementRequirement> {
    range_band_requirement(
        mover,
        target,
        RangeBand {
            minimum: 0.0,
            maximum: MOB_MELEE_MAX_RANGE,
            preferred: MOB_MELEE_APPROACH_RANGE,
        },
    )
}

/// Return a movement requirement that places the bot inside its ranged cast envelope.
pub fn mob_cast_approach_requirement(
    mover: WorldPosition,
    target: WorldPosition,
) -> Option<MovementRequirement> {
    range_band_requirement(
        mover,
        target,
        RangeBand {
            minimum: 0.0,
            maximum: MOB_CAST_MAX_RANGE,
            preferred: MOB_CAST_APPROACH_RANGE,
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeBand {
    pub minimum: f32,
    pub maximum: f32,
    pub preferred: f32,
}

/// Return the configured range band for a known movement spell.
pub fn spell_range_band(spell: u32, hostile_target: bool) -> RangeBand {
    wow_policy::combat::spells::range_for(spell, hostile_target)
        .map(|(minimum, maximum)| RangeBand {
            minimum,
            maximum,
            preferred: preferred_approach_range(minimum, maximum),
        })
        .unwrap_or(RangeBand {
            minimum: 0.0,
            maximum: MOB_CAST_MAX_RANGE,
            preferred: MOB_CAST_APPROACH_RANGE,
        })
}

/// Return a movement requirement that places the bot inside a spell's range band.
pub fn targeted_spell_approach_requirement(
    mover: WorldPosition,
    target: WorldPosition,
    spell: u32,
    hostile_target: bool,
) -> Option<MovementRequirement> {
    range_band_requirement(mover, target, spell_range_band(spell, hostile_target))
}

/// Return a movement requirement that places the bot in the target's rear arc.
pub fn mob_behind_approach_requirement(
    mover: WorldPosition,
    target: WorldPosition,
) -> Option<MovementRequirement> {
    if mover.map != target.map {
        return None;
    }
    let destination = target_approach_point(target, PI, MOB_REAR_STANDOFF)?;
    (mover.point.distance(destination) > MOB_REAR_TOLERANCE).then_some(MovementRequirement {
        destination,
        acceptable_range: MOB_REAR_TOLERANCE,
    })
}

fn preferred_approach_range(minimum: f32, maximum: f32) -> f32 {
    (maximum - 2.0).max(minimum).min(maximum)
}

fn range_band_requirement(
    mover: WorldPosition,
    target: WorldPosition,
    band: RangeBand,
) -> Option<MovementRequirement> {
    if mover.map != target.map
        || !mover.point.is_finite()
        || !target.point.is_finite()
        || !band.minimum.is_finite()
        || !band.maximum.is_finite()
        || !band.preferred.is_finite()
        || band.minimum < 0.0
        || band.maximum < band.minimum
        || band.preferred < band.minimum
        || band.preferred > band.maximum
    {
        return None;
    }
    let distance = mover.point.distance(target.point);
    if distance > band.maximum {
        Some(MovementRequirement {
            destination: target.point,
            acceptable_range: band.preferred,
        })
    } else if distance < band.minimum {
        let dx = mover.point.x - target.point.x;
        let dy = mover.point.y - target.point.y;
        let horizontal_distance = dx.hypot(dy);
        let retreat_range = (band.minimum + 2.0).min((band.minimum + band.maximum) / 2.0);
        let vertical_distance = mover.point.z - target.point.z;
        let horizontal_retreat = (retreat_range * retreat_range
            - vertical_distance * vertical_distance)
            .max(0.0)
            .sqrt();
        let (ux, uy) = if horizontal_distance < 0.001 {
            if !mover.orientation.is_finite() {
                return None;
            }
            (mover.orientation.cos(), mover.orientation.sin())
        } else {
            (dx / horizontal_distance, dy / horizontal_distance)
        };
        Some(MovementRequirement {
            destination: Vec3::new(
                target.point.x + ux * horizontal_retreat,
                target.point.y + uy * horizontal_retreat,
                mover.point.z,
            ),
            acceptable_range: 0.75,
        })
    } else {
        None
    }
}

/// Return a safe interaction point in the direction an NPC faces.
pub fn npc_front_approach_point(target: WorldPosition, standoff: f32) -> Option<Vec3> {
    target_approach_point(target, 0.0, standoff)
}

fn target_approach_point(target: WorldPosition, angle_offset: f32, standoff: f32) -> Option<Vec3> {
    if !target.point.is_finite()
        || !target.orientation.is_finite()
        || !angle_offset.is_finite()
        || !standoff.is_finite()
    {
        return None;
    }
    let orientation = target.orientation + angle_offset;
    Some(Vec3::new(
        target.point.x + orientation.cos() * standoff,
        target.point.y + orientation.sin() * standoff,
        target.point.z,
    ))
}

fn requires_npc_front_approach(command: &GameplayCommand) -> bool {
    matches!(
        command,
        GameplayCommand::Interact(_)
            | GameplayCommand::AcceptQuest { .. }
            | GameplayCommand::TurnInQuest { .. }
            | GameplayCommand::RequestQuestReward { .. }
            | GameplayCommand::ChooseQuestReward { .. }
            | GameplayCommand::VendorBuy { .. }
            | GameplayCommand::VendorSell { .. }
    )
}

pub fn movement_requirement(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Option<MovementRequirement> {
    let profile = profile(command)?;
    let mover = active_mover(snapshot)?;
    let target = target_position(snapshot, profile.target)?;
    if mover.map != target.map {
        return None;
    }
    if requires_npc_front_approach(command) {
        let destination = npc_front_approach_point(target, NPC_FRONT_STANDOFF)?;
        (mover.point.distance(destination) > NPC_FRONT_TOLERANCE).then_some(MovementRequirement {
            destination,
            acceptable_range: NPC_FRONT_TOLERANCE,
        })
    } else if matches!(command, GameplayCommand::Attack(_)) {
        mob_melee_approach_requirement(mover, target)
    } else if let GameplayCommand::Cast {
        spell,
        target: Some(_),
    } = command
    {
        let hostile_target = snapshot
            .state
            .entities
            .0
            .get(&profile.target)
            .is_some_and(|entity| entity.hostile);
        targeted_spell_approach_requirement(mover, target, *spell, hostile_target)
    } else if matches!(
        command,
        GameplayCommand::MaintainBuff { .. }
            | GameplayCommand::VehicleCast {
                target: Some(_),
                ..
            }
    ) {
        mob_cast_approach_requirement(mover, target)
    } else {
        range_band_requirement(
            mover,
            target,
            RangeBand {
                minimum: profile.minimum_range,
                maximum: profile.maximum_range,
                preferred: profile.approach_range,
            },
        )
    }
}

pub fn facing_requirement(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Option<FacingRequirement> {
    let profile = profile(command)?;
    let tolerance = profile.facing_tolerance?;
    let mover = active_mover(snapshot)?;
    let target = target_position(snapshot, profile.target)?;
    if mover.map != target.map {
        return None;
    }
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
    if !profile.requires_line_of_sight {
        return None;
    }
    (status(profile.target) == LineOfSightStatus::Blocked).then_some(LineOfSightRequirement {
        target: profile.target,
    })
}

pub fn desired_orientation(from: Vec3, to: Vec3) -> Option<f32> {
    if !from.is_finite() || !to.is_finite() {
        return None;
    }
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    if dx.abs() < f32::EPSILON && dy.abs() < f32::EPSILON {
        return None;
    }
    Some(dy.atan2(dx).rem_euclid(TAU))
}

pub fn angular_distance(a: f32, b: f32) -> f32 {
    if !a.is_finite() || !b.is_finite() {
        return f32::INFINITY;
    }
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d)
}

fn horizontal_direction(from: WorldPosition, to: WorldPosition) -> Option<(f32, f32)> {
    if from.map != to.map || !from.point.is_finite() || !to.point.is_finite() {
        return None;
    }
    let dx = to.point.x - from.point.x;
    let dy = to.point.y - from.point.y;
    let length = dx.hypot(dy);
    if length < 0.001 {
        return None;
    }
    Some((dx / length, dy / length))
}

pub fn server_range_reposition(
    mover: WorldPosition,
    target: WorldPosition,
    correction: ServerRangeCorrection,
    attempt: u8,
) -> Option<Vec3> {
    let (ux, uy) = horizontal_direction(mover, target)?;
    let step = 2.5 + f32::from(attempt.min(3));
    let sign = match correction {
        ServerRangeCorrection::MoveCloser => 1.0,
        ServerRangeCorrection::MoveFarther => -1.0,
    };
    Some(Vec3::new(
        mover.point.x + ux * step * sign,
        mover.point.y + uy * step * sign,
        mover.point.z,
    ))
}

/// Deterministic lateral candidate used after an authoritative server LOS
/// failure. The caller still routes this point through the shared movement
/// controller, so terrain/flying rules remain centralized.
pub fn line_of_sight_reposition(
    mover: WorldPosition,
    target: WorldPosition,
    attempt: u8,
) -> Option<Vec3> {
    let (ux, uy) = horizontal_direction(mover, target)?;
    let px = -uy;
    let py = ux;
    let side = if attempt % 2 == 0 { 1.0 } else { -1.0 };
    let radius = 3.0 + f32::from(attempt.min(3));
    Some(Vec3::new(
        mover.point.x + px * radius * side,
        mover.point.y + py * radius * side,
        mover.point.z,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{EntityId, GameplayCommand, Vec3, WorldPosition};
    use wow_state::{
        AuthoritativeState, Snapshot,
        entities::{EntityKind, EntityState},
    };

    fn snapshot(mover: WorldPosition, target: WorldPosition) -> Snapshot {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(mover);
        state.entities.0.insert(
            EntityId(7),
            EntityState {
                id: EntityId(7),
                entry: 1,
                kind: EntityKind::Unit,
                name: None,
                position: Some(target),
                health: None,
                hostile: true,
                interactable: true,
                ..Default::default()
            },
        );
        Snapshot::from_state(&state)
    }

    #[test]
    fn range_and_facing_are_shared_for_targeted_actions() {
        let s = snapshot(
            WorldPosition {
                map: 1,
                point: Vec3::new(0.0, 0.0, 0.0),
                orientation: PI,
            },
            WorldPosition {
                map: 1,
                point: Vec3::new(10.0, 0.0, 0.0),
                orientation: 0.0,
            },
        );
        let command = GameplayCommand::Attack(EntityId(7));
        assert!(movement_requirement(&s, &command).is_some());
        assert!(facing_requirement(&s, &command).is_some());
    }

    #[test]
    fn interaction_approach_keeps_margin_inside_server_range() {
        let s = snapshot(
            WorldPosition {
                map: 1,
                point: Vec3::new(0.0, 0.0, 0.0),
                orientation: 0.0,
            },
            WorldPosition {
                map: 1,
                point: Vec3::new(5.1, 0.0, 0.6),
                orientation: 0.0,
            },
        );
        let requirement = movement_requirement(&s, &GameplayCommand::Loot(EntityId(7))).unwrap();
        assert_eq!(requirement.acceptable_range, 4.0);
    }

    #[test]
    fn authoritative_range_recovery_moves_in_the_requested_direction() {
        let mover = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 5.0),
            orientation: 0.0,
        };
        let target = WorldPosition {
            map: 1,
            point: Vec3::new(10.0, 0.0, 5.0),
            orientation: 0.0,
        };
        let closer =
            server_range_reposition(mover, target, ServerRangeCorrection::MoveCloser, 0).unwrap();
        let farther =
            server_range_reposition(mover, target, ServerRangeCorrection::MoveFarther, 0).unwrap();
        assert!(closer.x > mover.point.x);
        assert!(farther.x < mover.point.x);
    }

    #[test]
    fn los_reposition_is_deterministic_and_alternates_side() {
        let mover = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 5.0),
            orientation: 0.0,
        };
        let target = WorldPosition {
            map: 1,
            point: Vec3::new(10.0, 0.0, 5.0),
            orientation: 0.0,
        };
        let a = line_of_sight_reposition(mover, target, 0).unwrap();
        let b = line_of_sight_reposition(mover, target, 1).unwrap();
        assert!(a.y > 0.0 && b.y < 0.0);
        assert_eq!(a.z, mover.point.z);
    }
}
