use wow_state::Snapshot;

/// Check that the player can do work that needs a stationary player on foot.
/// An unknown transport state is accepted, but a known attachment is not.
pub(crate) fn player_can_do_stationary_work(snapshot: &Snapshot) -> bool {
    !snapshot.state.position.moving
        && snapshot.state.control.mover.is_none()
        && snapshot.state.transport.attached != Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::EntityId;

    #[test]
    fn stationary_work_requires_no_motion_control_or_known_transport() {
        for (moving, mover, attached, expected) in [
            (false, None, None, true),
            (false, None, Some(false), true),
            (true, None, Some(false), false),
            (false, Some(EntityId(7)), Some(false), false),
            (false, None, Some(true), false),
        ] {
            let mut state = wow_state::AuthoritativeState::default();
            state.position.moving = moving;
            state.control.mover = mover;
            state.transport.attached = attached;
            assert_eq!(
                player_can_do_stationary_work(&Snapshot::from_state(&state)),
                expected
            );
        }
    }
}
