use wow_domain::{EntityId, GroupRole};
use wow_state::Snapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullDecision {
    Engage,
    WaitForGroupEngagement,
    WaitForThreatDelay,
}
#[derive(Clone, Debug, Default)]
pub struct EncounterIntent {
    pub preferred_target: Option<EntityId>,
    pub allowed_targets: Vec<EntityId>,
    pub interrupt_allowed: bool,
    pub hold_threat: bool,
}
pub fn from_observed(state: &Snapshot) -> EncounterIntent {
    let target = state.state.group.encounter_target;
    EncounterIntent {
        preferred_target: target,
        allowed_targets: target.into_iter().collect(),
        interrupt_allowed: target.is_some(),
        hold_threat: false,
    }
}

/// Authorize combat only for the current observed group encounter target.
/// Tanks may initiate that assigned encounter. Other roles wait until the
/// target is visibly in combat and targeting an observed online member.
pub fn pull_decision(
    state: &Snapshot,
    role: GroupRole,
    player: Option<EntityId>,
    target: EntityId,
    threat_delay_elapsed: bool,
) -> PullDecision {
    if !state
        .state
        .group
        .encounter_target
        .is_some_and(|authorized| authorized == target)
    {
        return PullDecision::WaitForGroupEngagement;
    }

    if role == GroupRole::Tank {
        return PullDecision::Engage;
    }

    if !is_group_engaged(state, player, target) {
        return PullDecision::WaitForGroupEngagement;
    }
    if threat_delay_elapsed {
        PullDecision::Engage
    } else {
        PullDecision::WaitForThreatDelay
    }
}

/// Confirm the assigned target is visibly fighting the player or an online
/// group member. Unknown combat or victim state cannot start the delay.
pub fn is_group_engaged(state: &Snapshot, player: Option<EntityId>, target: EntityId) -> bool {
    if !state
        .state
        .group
        .encounter_target
        .is_some_and(|authorized| authorized == target)
    {
        return false;
    }
    let Some(target_state) = state.state.entities.0.get(&target) else {
        return false;
    };
    let targets_group_member = target_state.target.is_some_and(|victim| {
        Some(victim) == player || crate::group::state::observed_member(state, victim)
    });
    target_state.hostile
        && !target_state.is_dead()
        && target_state.in_combat() == Some(true)
        && targets_group_member
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, entities::EntityState, group::GroupMember};

    fn encounter(
        role: GroupRole,
        target_victim: Option<EntityId>,
        in_combat: Option<bool>,
        threat_delay_elapsed: bool,
    ) -> PullDecision {
        let target = EntityId(9);
        let member = EntityId(2);
        let mut state = AuthoritativeState::default();
        state.group.encounter_target = Some(target);
        state.group.members.push(GroupMember {
            entity: member,
            online: true,
            ..Default::default()
        });
        state.entities.0.insert(
            target,
            EntityState {
                id: target,
                hostile: true,
                target: target_victim,
                unit_flags: in_combat.map(|active| if active { 0x0008_0000 } else { 0 }),
                ..Default::default()
            },
        );
        pull_decision(
            &Snapshot::from_state(&state),
            role,
            Some(EntityId(1)),
            target,
            threat_delay_elapsed,
        )
    }

    #[test]
    fn tank_can_initiate_only_the_observed_group_encounter_target() {
        assert_eq!(
            encounter(GroupRole::Tank, None, Some(false), false),
            PullDecision::Engage
        );
    }

    #[test]
    fn non_tank_waits_until_target_is_in_combat_against_observed_member() {
        assert_eq!(
            encounter(GroupRole::Ranged, None, Some(false), false),
            PullDecision::WaitForGroupEngagement
        );
        assert_eq!(
            encounter(GroupRole::Ranged, Some(EntityId(2)), Some(true), false),
            PullDecision::WaitForThreatDelay
        );
        assert_eq!(
            encounter(GroupRole::Ranged, Some(EntityId(2)), Some(true), true),
            PullDecision::Engage
        );
    }

    #[test]
    fn non_tank_does_not_infer_authority_from_combat_flag_alone() {
        assert_eq!(
            encounter(GroupRole::Healer, Some(EntityId(88)), Some(true), true),
            PullDecision::WaitForGroupEngagement
        );
        assert_eq!(
            encounter(GroupRole::Healer, Some(EntityId(2)), None, true),
            PullDecision::WaitForGroupEngagement
        );
    }

    #[test]
    fn non_tank_cannot_assist_a_target_outside_the_observed_encounter() {
        let target = EntityId(8);
        let mut state = AuthoritativeState::default();
        state.group.encounter_target = Some(EntityId(9));
        state.entities.0.insert(
            target,
            EntityState {
                id: target,
                hostile: true,
                target: Some(EntityId(2)),
                unit_flags: Some(0x0008_0000),
                ..Default::default()
            },
        );
        assert_eq!(
            pull_decision(
                &Snapshot::from_state(&state),
                GroupRole::Melee,
                Some(EntityId(1)),
                target,
                true,
            ),
            PullDecision::WaitForGroupEngagement
        );
    }
}
