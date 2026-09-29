use wow_domain::EntityId;
use wow_state::{Snapshot, entities::EntityKind};

/// Select the nearest observed hostile player on a supported battleground map.
/// Battleground mission permission is checked by the caller and again by final
/// action validation. Unknown positions and cross-map targets are ignored.
pub fn select_hostile_player_target(snapshot: &Snapshot) -> Option<EntityId> {
    let player_position = snapshot.state.position.player?;
    if !super::is_wotlk_battleground_map(player_position.map) {
        return None;
    }

    crate::selection::nearest_entity_id(snapshot.state.entities.0.iter().filter_map(
        |(id, entity)| {
            if entity.kind != EntityKind::Player || !entity.hostile || entity.is_dead() {
                return None;
            }
            let position = entity.position?;
            (position.map == player_position.map)
                .then_some((*id, player_position.point.distance(position.point)))
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{EntityId, Vec3, WorldPosition};
    use wow_state::entities::{EntityKind, EntityState};

    fn player_position(map: u32) -> WorldPosition {
        WorldPosition {
            map,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        }
    }

    #[test]
    fn selects_only_nearest_hostile_player_on_supported_battleground_map() {
        let mut state = wow_state::AuthoritativeState::default();
        state.position.player = Some(player_position(489));
        for (id, kind, hostile, map, x) in [
            (1, EntityKind::Player, true, 489, 9.0),
            (2, EntityKind::Player, true, 489, 4.0),
            (3, EntityKind::Player, false, 489, 1.0),
            (4, EntityKind::Unit, true, 489, 0.5),
            (5, EntityKind::Player, true, 0, 0.1),
        ] {
            state.entities.0.insert(
                EntityId(id),
                EntityState {
                    id: EntityId(id),
                    kind,
                    hostile,
                    position: Some(WorldPosition {
                        point: Vec3 { x, y: 0.0, z: 0.0 },
                        ..player_position(map)
                    }),
                    ..EntityState::default()
                },
            );
        }
        assert_eq!(
            select_hostile_player_target(&Snapshot::from_state(&state)),
            Some(EntityId(2))
        );
    }

    #[test]
    fn does_not_select_proactive_player_targets_outside_battleground_maps() {
        let mut state = wow_state::AuthoritativeState::default();
        state.position.player = Some(player_position(0));
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                hostile: true,
                position: Some(player_position(0)),
                ..EntityState::default()
            },
        );
        assert_eq!(
            select_hostile_player_target(&Snapshot::from_state(&state)),
            None
        );
    }
}
