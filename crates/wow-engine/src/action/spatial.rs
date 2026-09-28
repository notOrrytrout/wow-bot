use std::f32::consts::{PI, TAU};

const MELEE_FACING_TOLERANCE: f32 = PI / 6.0;
const INTERACTION_FACING_TOLERANCE: f32 = PI / 4.0;
const CAST_FACING_TOLERANCE: f32 = PI / 9.0;
const INTERACTION_MAX_RANGE: f32 = 5.0;
const INTERACTION_APPROACH_RANGE: f32 = 4.0;
// Keep the corpse approach inside the server interaction limit.
const CORPSE_LOOT_SAFE_RANGE: f32 = 2.5;
const NPC_FRONT_STANDOFF: f32 = 3.0;
const NPC_FRONT_TOLERANCE: f32 = 0.8;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorpseLootCheck {
    Ready,
    MissingTarget,
    NotCreature,
    NotDead,
    MissingPosition,
    MissingMover,
    DifferentMap,
    OutOfRange,
}

/// Check the authoritative state that AzerothCore requires before it accepts
/// a corpse loot request. Use a conservative 3-D center distance so vertical
/// separation cannot pass a ground-only range check by mistake.
pub fn check_corpse_loot(snapshot: &Snapshot, target: EntityId) -> CorpseLootCheck {
    let Some(entity) = snapshot.state.entities.0.get(&target) else {
        return CorpseLootCheck::MissingTarget;
    };
    if entity.kind != wow_state::entities::EntityKind::Unit {
        return CorpseLootCheck::NotCreature;
    }
    if !entity.is_dead() {
        return CorpseLootCheck::NotDead;
    }
    let Some(position) = entity.position else {
        return CorpseLootCheck::MissingPosition;
    };
    let Some(mover) = active_mover(snapshot) else {
        return CorpseLootCheck::MissingMover;
    };
    if mover.map != position.map {
        return CorpseLootCheck::DifferentMap;
    }
    if mover.point.distance(position.point) > CORPSE_LOOT_SAFE_RANGE {
        return CorpseLootCheck::OutOfRange;
    }
    CorpseLootCheck::Ready
}

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
        GameplayCommand::Loot(target) => (
            *target,
            0.0,
            INTERACTION_MAX_RANGE,
            CORPSE_LOOT_SAFE_RANGE,
            None,
            false,
        ),
        GameplayCommand::Interact(target)
        | GameplayCommand::UseGameObject(target)
        | GameplayCommand::CastGameObject { target, .. }
        | GameplayCommand::Gather(target)
        | GameplayCommand::EnterVehicle(target)
        | GameplayCommand::UseItem {
            target: Some(target),
            ..
        }
        | GameplayCommand::UseItemInstance {
            target: Some(target),
            ..
        }
        | GameplayCommand::AcceptQuest { giver: target, .. }
        | GameplayCommand::TurnInQuest { giver: target, .. }
        | GameplayCommand::RequestQuestReward { giver: target, .. }
        | GameplayCommand::ChooseQuestReward { giver: target, .. }
        | GameplayCommand::VendorBuy { vendor: target, .. }
        | GameplayCommand::VendorList { vendor: target }
        | GameplayCommand::VendorSell { vendor: target, .. } => (
            *target,
            0.0,
            INTERACTION_MAX_RANGE,
            INTERACTION_APPROACH_RANGE,
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
    range_band_requirement_with_metric(mover, target, band, true)
}

fn range_band_requirement_with_metric(
    mover: WorldPosition,
    target: WorldPosition,
    band: RangeBand,
    three_dimensional: bool,
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
    let horizontal_distance =
        (target.point.x - mover.point.x).hypot(target.point.y - mover.point.y);
    let vertical_distance = if three_dimensional {
        mover.point.z - target.point.z
    } else {
        0.0
    };
    let distance = if three_dimensional {
        mover.point.distance(target.point)
    } else {
        horizontal_distance
    };
    if distance > band.maximum {
        Some(MovementRequirement {
            destination: target.point,
            acceptable_range: band.preferred,
        })
    } else if distance < band.minimum {
        let retreat_range = (band.minimum + 2.0).min((band.minimum + band.maximum) / 2.0);
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
            (
                (mover.point.x - target.point.x) / horizontal_distance,
                (mover.point.y - target.point.y) / horizontal_distance,
            )
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
            | GameplayCommand::VendorList { .. }
            | GameplayCommand::VendorSell { .. }
    )
}

pub fn movement_requirement(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Option<MovementRequirement> {
    if matches!(
        command,
        GameplayCommand::MaintainBuff { target, .. }
            if snapshot.state.session.character_guid.map(EntityId) == Some(*target)
    ) {
        return None;
    }
    let profile = profile(command)?;
    let mover = active_mover(snapshot)?;
    let target = target_position(snapshot, profile.target)?;
    if mover.map != target.map {
        return None;
    }
    if requires_npc_front_approach(command) {
        let destination = npc_front_approach_point(target, NPC_FRONT_STANDOFF)?;
        (movement_distance(snapshot, mover.point, destination) > NPC_FRONT_TOLERANCE).then_some(
            MovementRequirement {
                destination,
                acceptable_range: NPC_FRONT_TOLERANCE,
            },
        )
    } else if matches!(command, GameplayCommand::Loot(_)) {
        loot_approach_requirement(mover, target, profile)
    } else if matches!(command, GameplayCommand::Attack(_)) {
        range_band_requirement_with_metric(
            mover,
            target,
            RangeBand {
                minimum: 0.0,
                maximum: MOB_MELEE_MAX_RANGE,
                preferred: MOB_MELEE_APPROACH_RANGE,
            },
            movement_uses_3d_distance(snapshot),
        )
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
        range_band_requirement_with_metric(
            mover,
            target,
            spell_range_band(*spell, hostile_target),
            movement_uses_3d_distance(snapshot),
        )
    } else if matches!(
        command,
        GameplayCommand::MaintainBuff { .. }
            | GameplayCommand::VehicleCast {
                target: Some(_),
                ..
            }
    ) {
        range_band_requirement_with_metric(
            mover,
            target,
            RangeBand {
                minimum: 0.0,
                maximum: MOB_CAST_MAX_RANGE,
                preferred: MOB_CAST_APPROACH_RANGE,
            },
            movement_uses_3d_distance(snapshot),
        )
    } else {
        range_band_requirement_with_metric(
            mover,
            target,
            RangeBand {
                minimum: profile.minimum_range,
                maximum: profile.maximum_range,
                preferred: profile.approach_range,
            },
            movement_uses_3d_distance(snapshot),
        )
    }
}

/// Report whether a targeted action is authoritatively in range. `None` means
/// the mover or target position is unavailable, or the action has no spatial profile.
pub fn is_in_range(snapshot: &Snapshot, command: &GameplayCommand) -> Option<bool> {
    let profile = profile(command)?;
    let mover = active_mover(snapshot)?;
    let target = target_position(snapshot, profile.target)?;
    if mover.map != target.map {
        return Some(false);
    }
    let distance = if matches!(command, GameplayCommand::Loot(_)) {
        mover.point.distance(target.point)
    } else {
        movement_distance(snapshot, mover.point, target.point)
    };
    let maximum_range = if matches!(command, GameplayCommand::Loot(_)) {
        profile.approach_range
    } else {
        profile.maximum_range
    };
    Some(distance >= profile.minimum_range && distance <= maximum_range)
}

fn loot_approach_requirement(
    mover: WorldPosition,
    target: WorldPosition,
    profile: SpatialProfile,
) -> Option<MovementRequirement> {
    if mover.map != target.map
        || !mover.point.is_finite()
        || !target.point.is_finite()
        || !profile.maximum_range.is_finite()
        || profile.maximum_range <= 0.0
    {
        return None;
    }

    let distance = mover.point.distance(target.point);
    if distance <= profile.approach_range {
        return None;
    }

    let vertical_distance = (mover.point.z - target.point.z).abs();
    if vertical_distance >= profile.maximum_range {
        return None;
    }
    let horizontal_limit = (profile.approach_range * profile.approach_range
        - vertical_distance * vertical_distance)
        .max(0.0)
        .sqrt();
    let acceptable_range = profile
        .approach_range
        .min((horizontal_limit - 0.25).max(0.25));

    Some(MovementRequirement {
        destination: target.point,
        acceptable_range,
    })
}

/// Match the lane movement controller's arrival metric for a spatial
/// prerequisite. Ground movement ignores Z; flight movement uses 3-D distance.
fn movement_uses_3d_distance(snapshot: &Snapshot) -> bool {
    let controlled_mover = snapshot.state.control.mover.is_some();
    let flags = if controlled_mover {
        snapshot.state.control.movement_flags
    } else {
        snapshot.state.position.flags
    };
    wow_navigation::LocomotionMode::from_server_flags(controlled_mover, flags)
        == wow_navigation::LocomotionMode::Flight
}

fn movement_distance(snapshot: &Snapshot, from: Vec3, to: Vec3) -> f32 {
    if movement_uses_3d_distance(snapshot) {
        from.distance(to)
    } else {
        (to.x - from.x).hypot(to.y - from.y)
    }
}

pub fn facing_requirement(
    snapshot: &Snapshot,
    command: &GameplayCommand,
) -> Option<FacingRequirement> {
    if matches!(
        command,
        GameplayCommand::MaintainBuff { target, .. }
            if snapshot.state.session.character_guid.map(EntityId) == Some(*target)
    ) {
        return None;
    }
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

    fn corpse_snapshot(target: WorldPosition) -> Snapshot {
        let mut snapshot = snapshot(
            WorldPosition {
                map: 1,
                point: Vec3::new(0.0, 0.0, 0.0),
                orientation: 0.0,
            },
            target,
        );
        snapshot
            .state
            .entities
            .0
            .get_mut(&EntityId(7))
            .unwrap()
            .health = Some((0, 100));
        snapshot
    }

    #[test]
    fn corpse_loot_checks_require_a_present_dead_unit_and_conservative_range() {
        let nearby = WorldPosition {
            map: 1,
            point: Vec3::new(2.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut snapshot = corpse_snapshot(nearby);
        assert_eq!(
            check_corpse_loot(&snapshot, EntityId(7)),
            CorpseLootCheck::Ready
        );

        let outside_safe_range = corpse_snapshot(WorldPosition {
            point: Vec3::new(4.0, 0.0, 0.0),
            ..nearby
        });
        assert_eq!(
            check_corpse_loot(&outside_safe_range, EntityId(7)),
            CorpseLootCheck::OutOfRange
        );

        snapshot.state.entities.0.remove(&EntityId(7));
        assert_eq!(
            check_corpse_loot(&snapshot, EntityId(7)),
            CorpseLootCheck::MissingTarget
        );

        let mut snapshot = corpse_snapshot(nearby);
        snapshot
            .state
            .entities
            .0
            .get_mut(&EntityId(7))
            .unwrap()
            .health = Some((1, 100));
        assert_eq!(
            check_corpse_loot(&snapshot, EntityId(7)),
            CorpseLootCheck::NotDead
        );

        let mut snapshot = corpse_snapshot(nearby);
        snapshot
            .state
            .entities
            .0
            .get_mut(&EntityId(7))
            .unwrap()
            .kind = EntityKind::Player;
        assert_eq!(
            check_corpse_loot(&snapshot, EntityId(7)),
            CorpseLootCheck::NotCreature
        );

        let mut snapshot = corpse_snapshot(nearby);
        snapshot
            .state
            .entities
            .0
            .get_mut(&EntityId(7))
            .unwrap()
            .position = None;
        assert_eq!(
            check_corpse_loot(&snapshot, EntityId(7)),
            CorpseLootCheck::MissingPosition
        );

        let mut snapshot = corpse_snapshot(nearby);
        snapshot.state.position.player = None;
        assert_eq!(
            check_corpse_loot(&snapshot, EntityId(7)),
            CorpseLootCheck::MissingMover
        );

        let snapshot = corpse_snapshot(WorldPosition {
            point: Vec3::new(0.0, 0.0, 5.1),
            ..nearby
        });
        assert_eq!(
            check_corpse_loot(&snapshot, EntityId(7)),
            CorpseLootCheck::OutOfRange
        );

        let snapshot = corpse_snapshot(WorldPosition { map: 2, ..nearby });
        assert_eq!(
            check_corpse_loot(&snapshot, EntityId(7)),
            CorpseLootCheck::DifferentMap
        );
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

        let loot = GameplayCommand::Loot(EntityId(7));
        assert_eq!(facing_requirement(&s, &loot), None);
        assert_eq!(
            line_of_sight_requirement(&loot, |_| LineOfSightStatus::Blocked),
            None
        );
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
        assert!(requirement.acceptable_range < CORPSE_LOOT_SAFE_RANGE);
        assert!(requirement.acceptable_range > 2.0);

        let near_server_limit = snapshot(
            WorldPosition {
                map: 1,
                point: Vec3::new(0.0, 0.0, 0.0),
                orientation: 0.0,
            },
            WorldPosition {
                map: 1,
                point: Vec3::new(4.0, 0.0, 0.0),
                orientation: 0.0,
            },
        );
        let command = GameplayCommand::Loot(EntityId(7));
        assert_eq!(is_in_range(&near_server_limit, &command), Some(false));
        assert!(movement_requirement(&near_server_limit, &command).is_some());
    }

    #[test]
    fn authoritative_range_status_distinguishes_arrival_from_missing_or_distant_state() {
        let mover = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let command = GameplayCommand::Loot(EntityId(7));
        let nearby = snapshot(
            mover,
            WorldPosition {
                map: 1,
                point: Vec3::new(2.0, 0.0, 0.0),
                orientation: 0.0,
            },
        );
        assert_eq!(is_in_range(&nearby, &command), Some(true));
        let distant = snapshot(
            mover,
            WorldPosition {
                map: 1,
                point: Vec3::new(10.0, 0.0, 0.0),
                orientation: 0.0,
            },
        );
        assert_eq!(is_in_range(&distant, &command), Some(false));

        let mut missing = wow_state::AuthoritativeState::default();
        missing.entities.0.insert(
            EntityId(7),
            EntityState {
                id: EntityId(7),
                kind: EntityKind::Unit,
                ..Default::default()
            },
        );
        assert_eq!(is_in_range(&Snapshot::from_state(&missing), &command), None);
    }

    #[test]
    fn npc_turn_in_approach_uses_eight_tenths_yard_tolerance() {
        let s = snapshot(
            WorldPosition {
                map: 1,
                point: Vec3::new(10.0, 0.0, 0.0),
                orientation: 0.0,
            },
            WorldPosition {
                map: 1,
                point: Vec3::new(0.0, 0.0, 0.0),
                orientation: 0.0,
            },
        );
        let requirement = movement_requirement(
            &s,
            &GameplayCommand::TurnInQuest {
                quest: 170,
                giver: EntityId(7),
            },
        )
        .expect("NPC front approach is required");

        assert_eq!(requirement.destination, Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(requirement.acceptable_range, 0.8);
    }

    #[test]
    fn ground_npc_approach_uses_same_xy_arrival_envelope_as_movement() {
        let mut s = snapshot(
            WorldPosition {
                map: 1,
                point: Vec3::new(3.0, 0.799_881_756, 2.5),
                orientation: 0.0,
            },
            WorldPosition {
                map: 1,
                point: Vec3::new(0.0, 0.0, 0.0),
                orientation: 0.0,
            },
        );
        let command = GameplayCommand::TurnInQuest {
            quest: 170,
            giver: EntityId(7),
        };

        // Ground movement reports arrival at horizontal distance 0.799881756,
        // even though the active mover has a vertical offset from the NPC.
        assert!(movement_requirement(&s, &command).is_none());

        // Flight remains 3-D: the same offset is outside the interaction envelope.
        s.state.control.mover = Some(EntityId(9));
        s.state.control.movement_flags = 0x0200_0000;
        assert!(movement_requirement(&s, &command).is_some());
    }

    #[test]
    fn ground_spell_range_uses_horizontal_distance_like_movement_arrival() {
        let band = spell_range_band(686, true);
        let horizontal = band.maximum - 1.0;
        let vertical = band.maximum;
        let mover = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let target = WorldPosition {
            map: 1,
            point: Vec3::new(horizontal, 0.0, vertical),
            orientation: 0.0,
        };
        let command = GameplayCommand::Cast {
            spell: 686,
            target: Some(EntityId(7)),
        };
        let mut s = snapshot(mover, target);

        assert!(horizontal <= band.maximum);
        assert!(mover.point.distance(target.point) > band.maximum);
        assert!(movement_requirement(&s, &command).is_none());

        s.state.control.mover = Some(EntityId(9));
        s.state.control.movement_flags = 0x0200_0000;
        assert!(movement_requirement(&s, &command).is_some());
    }

    #[test]
    fn self_buff_does_not_move_or_turn_toward_stale_self_position() {
        let mover = WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let stale_self_position = WorldPosition {
            map: 1,
            point: Vec3::new(21.5, 0.0, 0.0),
            orientation: 0.0,
        };
        let mut s = snapshot(mover, stale_self_position);
        s.state.session.character_guid = Some(7);
        let command = GameplayCommand::MaintainBuff {
            spell: 687,
            target: EntityId(7),
        };

        assert!(movement_requirement(&s, &command).is_none());
        assert!(facing_requirement(&s, &command).is_none());
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
