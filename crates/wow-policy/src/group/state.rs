use wow_domain::EntityId;
use wow_state::Snapshot;

pub fn online_members_except(
    state: &Snapshot,
    excluded: Option<EntityId>,
) -> impl Iterator<Item = EntityId> + '_ {
    state
        .state
        .group
        .members
        .iter()
        .filter(move |member| member.online && Some(member.entity) != excluded)
        .map(|member| member.entity)
}

pub fn observed_member(state: &Snapshot, id: EntityId) -> bool {
    online_members_except(state, None).any(|member| member == id)
}
pub fn leader(state: &Snapshot) -> Option<EntityId> {
    state.state.group.leader
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, group::GroupMember};

    #[test]
    fn online_member_filter_excludes_the_player_and_offline_members() {
        let mut state = AuthoritativeState::default();
        state.group.members = vec![
            GroupMember {
                entity: EntityId(1),
                online: true,
                ..Default::default()
            },
            GroupMember {
                entity: EntityId(2),
                online: true,
                ..Default::default()
            },
            GroupMember {
                entity: EntityId(3),
                online: false,
                ..Default::default()
            },
        ];
        let snapshot = Snapshot::from_state(&state);

        assert_eq!(
            online_members_except(&snapshot, Some(EntityId(1))).collect::<Vec<_>>(),
            vec![EntityId(2)]
        );
        assert_eq!(
            online_members_except(&snapshot, None).collect::<Vec<_>>(),
            vec![EntityId(1), EntityId(2)]
        );
    }
}
