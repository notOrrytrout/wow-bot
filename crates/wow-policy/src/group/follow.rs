use wow_domain::{EntityId, Vec3, WorldPosition};
use wow_state::Snapshot;

pub const GROUP_FOLLOW_STOP_DISTANCE: f32 = 5.0;

/// Return the current observed position to follow, preferring the group leader.
/// Positions from another map or absent entity state cannot guide movement.
pub fn target_position(state: &Snapshot, player: EntityId) -> Option<WorldPosition> {
    let group = &state.state.group;
    if group.encounter_target.is_some() {
        return None;
    }
    let candidates = group
        .leader
        .into_iter()
        .chain(group.members.iter().map(|member| member.entity))
        .filter(|id| *id != player)
        .filter(|id| {
            group
                .members
                .iter()
                .any(|member| member.entity == *id && member.online)
        });
    candidates
        .filter_map(|id| state.state.entities.0.get(&id)?.position)
        .find(|position| position.point.is_finite())
}

/// Calculate a point on the line to the member, keeping the requested gap.
pub fn follow_destination(from: WorldPosition, target: WorldPosition, stop: f32) -> Option<Vec3> {
    if from.map != target.map || !from.point.is_finite() || !target.point.is_finite() {
        return None;
    }
    let dx = target.point.x - from.point.x;
    let dy = target.point.y - from.point.y;
    let distance = dx.hypot(dy);
    if !distance.is_finite() || distance <= stop.max(0.0) || distance == 0.0 {
        return None;
    }
    let gap = stop.max(0.0).min(distance);
    Some(Vec3::new(
        target.point.x - dx / distance * gap,
        target.point.y - dy / distance * gap,
        target.point.z,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, entities::EntityKind, group::GroupMember};

    fn position(map: u32, x: f32, y: f32) -> WorldPosition {
        WorldPosition {
            map,
            point: Vec3::new(x, y, 0.0),
            orientation: 0.0,
        }
    }

    #[test]
    fn destination_tracks_moving_member_and_keeps_stop_distance() {
        let first = follow_destination(position(1, 0.0, 0.0), position(1, 20.0, 0.0), 5.0).unwrap();
        let moved = follow_destination(position(1, 0.0, 0.0), position(1, 0.0, 20.0), 5.0).unwrap();
        assert_eq!(first, Vec3::new(15.0, 0.0, 0.0));
        assert_eq!(moved, Vec3::new(0.0, 15.0, 0.0));
    }

    #[test]
    fn target_position_requires_online_observed_member_state() {
        let mut state = AuthoritativeState::default();
        state.group.members.push(GroupMember {
            entity: EntityId(2),
            online: true,
            ..Default::default()
        });
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(target_position(&snapshot, EntityId(1)), None);
        state.entities.0.insert(
            EntityId(2),
            wow_state::entities::EntityState {
                id: EntityId(2),
                kind: EntityKind::Player,
                position: Some(position(1, 8.0, 0.0)),
                ..Default::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(
            target_position(&snapshot, EntityId(1)),
            Some(position(1, 8.0, 0.0))
        );
    }

    #[test]
    fn follow_does_not_cross_map_or_move_inside_stop_distance() {
        assert_eq!(
            follow_destination(position(1, 0.0, 0.0), position(2, 20.0, 0.0), 5.0),
            None
        );
        assert_eq!(
            follow_destination(position(1, 0.0, 0.0), position(1, 4.0, 0.0), 5.0),
            None
        );
    }

    #[test]
    fn encounter_target_preempts_follow() {
        let mut state = AuthoritativeState::default();
        state.group.members.push(GroupMember {
            entity: EntityId(2),
            online: true,
            ..Default::default()
        });
        state.entities.0.insert(
            EntityId(2),
            wow_state::entities::EntityState {
                id: EntityId(2),
                position: Some(position(1, 20.0, 0.0)),
                ..Default::default()
            },
        );
        state.group.encounter_target = Some(EntityId(3));
        assert_eq!(
            target_position(&Snapshot::from_state(&state), EntityId(1)),
            None
        );
    }
}
