use wow_domain::EntityId;

/// Choose the nearest candidate and break equal-distance ties by entity ID.
pub(crate) fn nearest_entity_id(
    candidates: impl Iterator<Item = (EntityId, f32)>,
) -> Option<EntityId> {
    candidates
        .min_by(|(left_id, left_distance), (right_id, right_distance)| {
            left_distance
                .total_cmp(right_distance)
                .then_with(|| left_id.cmp(right_id))
        })
        .map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_candidate_uses_entity_id_for_distance_ties() {
        assert_eq!(
            nearest_entity_id(
                [(EntityId(8), 4.0), (EntityId(3), 4.0), (EntityId(9), 6.0)].into_iter()
            ),
            Some(EntityId(3))
        );
    }
}
