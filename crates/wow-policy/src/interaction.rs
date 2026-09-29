use wow_domain::WorldPosition;
use wow_state::entities::{EntityKind, EntityState};

/// Check that an observed interactable unit is within range on the same map.
pub fn is_nearby_interactable_unit(
    entity: &EntityState,
    from: WorldPosition,
    max_distance: f32,
) -> bool {
    entity.kind == EntityKind::Unit
        && entity.interactable
        && entity.position.is_some_and(|position| {
            position.map == from.map && position.point.distance(from.point) <= max_distance
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{EntityId, Vec3};

    fn position(map: u32, x: f32, y: f32, z: f32) -> WorldPosition {
        WorldPosition {
            map,
            point: Vec3::new(x, y, z),
            orientation: 0.0,
        }
    }

    #[test]
    fn nearby_interaction_requires_an_interactable_unit_on_the_same_map_and_in_range() {
        let from = position(1, 0.0, 0.0, 0.0);
        let mut entity = EntityState {
            id: EntityId(2),
            kind: EntityKind::Unit,
            interactable: true,
            position: Some(position(1, 3.0, 0.0, 0.0)),
            ..Default::default()
        };
        assert!(is_nearby_interactable_unit(&entity, from, 5.0));

        entity.position = Some(position(1, 6.0, 0.0, 0.0));
        assert!(!is_nearby_interactable_unit(&entity, from, 5.0));
        entity.position = Some(position(2, 1.0, 0.0, 0.0));
        assert!(!is_nearby_interactable_unit(&entity, from, 5.0));
        entity.position = Some(position(1, 1.0, 0.0, 0.0));
        entity.interactable = false;
        assert!(!is_nearby_interactable_unit(&entity, from, 5.0));
        entity.interactable = true;
        entity.kind = EntityKind::GameObject;
        assert!(!is_nearby_interactable_unit(&entity, from, 5.0));
    }
}
