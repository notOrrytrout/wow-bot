use wow_domain::EntityId;
use wow_state::{Snapshot, entities::EntityKind};

/// Keep voluntary Grind pulls within the legacy bounded observation horizon.
pub const GRIND_TARGET_MAX_DISTANCE_YARDS: f32 = 400.0;
const CLUSTER_RADIUS_YARDS: f32 = 9.0;

/// Select the nearest safe, exact-name hostile for a named Grind mission.
///
/// The current lane state does not expose creature elite/boss rank or an
/// observation timestamp. This selector does not infer either value. It uses
/// only retained, same-map entity observations and the latest positions; the
/// caller's action validation remains responsible for rechecking current state.
pub fn select_named_grind_target(snapshot: &Snapshot, creature: &str) -> Option<EntityId> {
    let creature = creature.trim();
    if creature.is_empty() || player_is_engaged(snapshot) {
        return None;
    }

    let player_id = snapshot.state.session.character_guid.map(EntityId)?;
    let player = snapshot.state.entities.0.get(&player_id)?;
    let player_position = snapshot
        .state
        .control
        .active_position(snapshot.state.position.player)?;
    if !player_position.point.is_finite() {
        return None;
    }

    if player.health.is_some_and(|(health, max_health)| {
        max_health > 0 && health.saturating_mul(100) < max_health.saturating_mul(45)
    }) {
        return None;
    }
    if player_mana(player).is_some_and(|(mana, max_mana)| {
        max_mana > 0 && mana.saturating_mul(100) < max_mana.saturating_mul(20)
    }) {
        return None;
    }

    crate::selection::nearest_entity_id(
        snapshot
            .state
            .entities
            .0
            .values()
            .filter(|entity| {
                entity.id != player_id
                    && entity.kind == EntityKind::Unit
                    && entity.hostile
                    && !entity.is_dead()
                    && entity.name.as_deref().is_some_and(|name| {
                        !name.trim().is_empty() && name.trim().eq_ignore_ascii_case(creature)
                    })
            })
            .filter_map(|target| {
                let position = target.position?;
                if position.map != player_position.map || !position.point.is_finite() {
                    return None;
                }
                let distance = player_position.point.distance(position.point);
                if !distance.is_finite() || distance > GRIND_TARGET_MAX_DISTANCE_YARDS {
                    return None;
                }
                if level_exceeds_safe_limit(player.level, target.level) {
                    return None;
                }
                if !pull_is_safe(snapshot, player, target) {
                    return None;
                }
                Some((target.id, distance))
            }),
    )
}

fn player_is_engaged(snapshot: &Snapshot) -> bool {
    let player = snapshot.state.session.character_guid.map(EntityId);
    player
        .and_then(|id| snapshot.state.entities.0.get(&id))
        .and_then(wow_state::entities::EntityState::in_combat)
        == Some(true)
        || crate::combat::engagement::survival_attacker(snapshot).is_some()
}

fn player_mana(player: &wow_state::entities::EntityState) -> Option<(u32, u32)> {
    if player.power_type == Some(0) {
        player.power
    } else {
        None
    }
}

fn level_exceeds_safe_limit(player_level: Option<u32>, target_level: Option<u32>) -> bool {
    matches!((player_level, target_level), (Some(player), Some(target)) if target > player.saturating_add(2))
}

fn pull_is_safe(
    snapshot: &Snapshot,
    player: &wow_state::entities::EntityState,
    target: &wow_state::entities::EntityState,
) -> bool {
    if crate::combat::engagement::is_attacking_player_or_group(snapshot, target.id) {
        // A defensive target belongs to self-defense policy, not voluntary Grind.
        return false;
    }

    let Some(target_position) = target.position else {
        return false;
    };
    let clustered = snapshot
        .state
        .entities
        .0
        .values()
        .filter(|other| {
            if other.id == target.id
                || other.kind != EntityKind::Unit
                || !other.hostile
                || other.is_dead()
            {
                return false;
            }
            let Some(position) = other.position else {
                return false;
            };
            if position.map != target_position.map || !position.point.is_finite() {
                return false;
            }
            position.point.distance(target_position.point) <= CLUSTER_RADIUS_YARDS
        })
        .count();
    if clustered >= 3 {
        return false;
    }
    if clustered == 0 {
        return true;
    }

    let health_ok = player.health.is_none_or(|(health, max_health)| {
        max_health == 0 || health.saturating_mul(100) >= max_health.saturating_mul(75)
    });
    let mana_ok = player_mana(player).is_none_or(|(mana, max_mana)| {
        max_mana == 0 || mana.saturating_mul(100) >= max_mana.saturating_mul(45)
    });
    health_ok && mana_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{Vec3, WorldPosition};
    use wow_state::{AuthoritativeState, entities::EntityState};

    fn position(x: f32) -> WorldPosition {
        WorldPosition {
            map: 1,
            point: Vec3::new(x, 0.0, 0.0),
            orientation: 0.0,
        }
    }

    fn snapshot() -> (AuthoritativeState, EntityId) {
        let player_id = EntityId(1);
        let mut state = AuthoritativeState::default();
        state.session.character_guid = Some(player_id.0);
        state.position.player = Some(position(0.0));
        state.entities.0.insert(
            player_id,
            EntityState {
                id: player_id,
                kind: EntityKind::Player,
                health: Some((100, 100)),
                power: Some((100, 100)),
                power_type: Some(0),
                level: Some(10),
                ..Default::default()
            },
        );
        (state, player_id)
    }

    fn hostile(state: &mut AuthoritativeState, id: u64, name: &str, x: f32) -> EntityId {
        let id = EntityId(id);
        state.entities.0.insert(
            id,
            EntityState {
                id,
                entry: 100,
                kind: EntityKind::Unit,
                name: Some(name.into()),
                position: Some(position(x)),
                health: Some((100, 100)),
                level: Some(10),
                hostile: true,
                ..Default::default()
            },
        );
        id
    }

    #[test]
    fn selects_nearest_exact_name_and_breaks_equal_distance_by_id() {
        let (mut state, _) = snapshot();
        let farther = hostile(&mut state, 3, "Wolf", 12.0);
        let nearer_high_id = hostile(&mut state, 9, "wolf", 7.0);
        let nearer_low_id = hostile(&mut state, 2, "WOLF", -7.0);
        hostile(&mut state, 1_000, "Dire Wolf", 1.0);
        let snapshot = Snapshot::from_state(&state);

        assert_eq!(
            select_named_grind_target(&snapshot, " Wolf "),
            Some(nearer_low_id)
        );
        assert_ne!(nearer_high_id, farther);
    }

    #[test]
    fn rejects_missing_position_wrong_map_non_finite_or_distant_targets() {
        let (mut state, _) = snapshot();
        let missing_position = hostile(&mut state, 2, "Wolf", 10.0);
        state
            .entities
            .0
            .get_mut(&missing_position)
            .unwrap()
            .position = None;
        let horizon_target = hostile(&mut state, 3, "Wolf", 400.0);
        let distant = hostile(&mut state, 6, "Wolf", 401.0);
        let wrong_map = hostile(&mut state, 4, "Wolf", 5.0);
        state
            .entities
            .0
            .get_mut(&wrong_map)
            .unwrap()
            .position
            .as_mut()
            .unwrap()
            .map = 2;
        let non_finite = hostile(&mut state, 5, "Wolf", f32::NAN);
        assert_eq!(
            select_named_grind_target(&Snapshot::from_state(&state), "Wolf"),
            Some(horizon_target),
            "the legacy 400-yard observation boundary is inclusive"
        );

        state.entities.0.remove(&horizon_target);
        state.entities.0.remove(&distant);
        state.entities.0.remove(&wrong_map);
        state.entities.0.remove(&non_finite);
        assert!(select_named_grind_target(&Snapshot::from_state(&state), "Wolf").is_none());
    }

    #[test]
    fn rejects_dead_player_non_hostile_and_level_unsafe_targets_but_allows_unknown_level() {
        let (mut state, player) = snapshot();
        let dead = hostile(&mut state, 2, "Wolf", 2.0);
        state.entities.0.get_mut(&dead).unwrap().health = Some((0, 100));
        let too_high = hostile(&mut state, 3, "Wolf", 3.0);
        state.entities.0.get_mut(&too_high).unwrap().level = Some(13);
        let non_hostile = hostile(&mut state, 4, "Wolf", 4.0);
        state.entities.0.get_mut(&non_hostile).unwrap().hostile = false;
        let player_entity = EntityId(5);
        state.entities.0.insert(
            player_entity,
            EntityState {
                id: player_entity,
                kind: EntityKind::Player,
                name: Some("Wolf".into()),
                position: Some(position(5.0)),
                health: Some((100, 100)),
                hostile: true,
                ..Default::default()
            },
        );
        assert!(select_named_grind_target(&Snapshot::from_state(&state), "Wolf").is_none());

        state.entities.0.remove(&non_hostile);
        state.entities.0.remove(&player_entity);
        state.entities.0.remove(&dead);
        state.entities.0.get_mut(&player).unwrap().level = None;
        assert_eq!(
            select_named_grind_target(&Snapshot::from_state(&state), "Wolf"),
            Some(too_high),
            "missing player level leaves the old relative-level limit unapplied"
        );

        state.entities.0.get_mut(&too_high).unwrap().level = None;
        assert_eq!(
            select_named_grind_target(&Snapshot::from_state(&state), "Wolf"),
            Some(too_high),
            "unknown target level is allowed; entity rank is not available to infer"
        );
    }

    #[test]
    fn rejects_low_health_or_mana_and_allows_unknown_health_and_mana() {
        let (mut state, player) = snapshot();
        let target = hostile(&mut state, 2, "Wolf", 2.0);
        state.entities.0.get_mut(&player).unwrap().health = Some((44, 100));
        assert!(select_named_grind_target(&Snapshot::from_state(&state), "Wolf").is_none());
        state.entities.0.get_mut(&player).unwrap().health = Some((100, 100));
        state.entities.0.get_mut(&player).unwrap().power = Some((19, 100));
        assert!(select_named_grind_target(&Snapshot::from_state(&state), "Wolf").is_none());
        state.entities.0.get_mut(&player).unwrap().power = None;
        assert_eq!(
            select_named_grind_target(&Snapshot::from_state(&state), "Wolf"),
            Some(target)
        );
    }

    #[test]
    fn rejects_unsafe_cluster_and_requires_more_reserves_for_small_cluster() {
        let (mut state, player) = snapshot();
        let target = hostile(&mut state, 2, "Wolf", 4.0);
        hostile(&mut state, 3, "Boar", 5.0);
        hostile(&mut state, 4, "Boar", 6.0);
        hostile(&mut state, 5, "Boar", 7.0);
        let snapshot = Snapshot::from_state(&state);
        assert!(select_named_grind_target(&snapshot, "Wolf").is_none());

        state.entities.0.remove(&EntityId(5));
        state.entities.0.get_mut(&player).unwrap().health = Some((74, 100));
        assert!(select_named_grind_target(&Snapshot::from_state(&state), "Wolf").is_none());
        state.entities.0.get_mut(&player).unwrap().health = Some((75, 100));
        state.entities.0.get_mut(&player).unwrap().power = Some((44, 100));
        assert!(select_named_grind_target(&Snapshot::from_state(&state), "Wolf").is_none());
        state.entities.0.get_mut(&player).unwrap().power = Some((45, 100));
        assert_eq!(
            select_named_grind_target(&Snapshot::from_state(&state), "Wolf"),
            Some(target)
        );
    }

    #[test]
    fn grind_does_not_take_over_engaged_or_self_defense_combat() {
        let (mut state, player) = snapshot();
        hostile(&mut state, 2, "Wolf", 2.0);
        state.entities.0.get_mut(&player).unwrap().unit_flags = Some(0x0008_0000);
        assert!(select_named_grind_target(&Snapshot::from_state(&state), "Wolf").is_none());

        state.entities.0.get_mut(&player).unwrap().unit_flags = Some(0);
        let attacker = hostile(&mut state, 3, "Boar", 2.0);
        state.entities.0.get_mut(&attacker).unwrap().target = Some(player);
        assert!(select_named_grind_target(&Snapshot::from_state(&state), "Wolf").is_none());
    }
}
