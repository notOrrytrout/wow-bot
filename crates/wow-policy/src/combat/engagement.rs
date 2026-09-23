use std::collections::BTreeSet;
use wow_domain::EntityId;
use wow_state::{entities::EntityKind, Snapshot};

/// Authoritative GUIDs whose attackers are part of the bot's active encounter.
/// PvP players are intentionally excluded from automatic self-defense here;
/// player-vs-player policy is a separate subsystem.
pub fn protected_guids(snapshot: &Snapshot) -> BTreeSet<EntityId> {
    let mut protected = BTreeSet::new();
    if let Some(player) = snapshot.state.session.character_guid.map(EntityId) {
        protected.insert(player);
    }
    if let Some(mover) = snapshot.state.control.mover {
        protected.insert(mover);
    }
    for member in &snapshot.state.group.members {
        if member.online {
            protected.insert(member.entity);
        }
    }
    protected
}

pub fn is_attacking_player_or_group(snapshot: &Snapshot, entity: EntityId) -> bool {
    let Some(attacker) = snapshot.state.entities.0.get(&entity) else { return false; };
    if attacker.kind != EntityKind::Unit { return false; }
    if attacker.health.is_some_and(|(current, _)| current == 0) { return false; }
    attacker.target.is_some_and(|target| protected_guids(snapshot).contains(&target))
}

pub fn survival_attacker(snapshot: &Snapshot) -> Option<EntityId> {
    let player_position = snapshot.state.control.active_position(snapshot.state.position.player).map(|p| p.point);
    snapshot.state.entities.0.iter()
        .filter(|(id, _)| is_attacking_player_or_group(snapshot, **id))
        .min_by(|(a_id, a), (b_id, b)| {
            let distance = |entity: &wow_state::entities::EntityState| {
                match (player_position, entity.position) {
                    (Some(player), Some(position)) => player.distance(position.point),
                    _ => f32::INFINITY,
                }
            };
            distance(a).total_cmp(&distance(b)).then_with(|| a_id.cmp(b_id))
        })
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, entities::EntityState, group::GroupMember};

    #[test]
    fn npc_targeting_player_is_survival_attacker_even_without_pull_ownership() {
        let mut state = AuthoritativeState::default();
        state.session.character_guid = Some(1);
        state.entities.0.insert(EntityId(9), EntityState { id: EntityId(9), kind: EntityKind::Unit, health: Some((10,10)), target: Some(EntityId(1)), ..Default::default() });
        assert_eq!(survival_attacker(&Snapshot::from_state(&state)), Some(EntityId(9)));
    }

    #[test]
    fn npc_targeting_online_group_member_is_engaged() {
        let mut state = AuthoritativeState::default();
        state.session.character_guid = Some(1);
        state.group.members.push(GroupMember { entity: EntityId(2), name: "ally".into(), role: None, online: true });
        state.entities.0.insert(EntityId(9), EntityState { id: EntityId(9), kind: EntityKind::Unit, health: Some((10,10)), target: Some(EntityId(2)), ..Default::default() });
        assert!(is_attacking_player_or_group(&Snapshot::from_state(&state), EntityId(9)));
    }

    #[test]
    fn player_merely_targeting_bot_is_not_auto_attacked() {
        let mut state = AuthoritativeState::default();
        state.session.character_guid = Some(1);
        state.entities.0.insert(EntityId(9), EntityState { id: EntityId(9), kind: EntityKind::Player, health: Some((10,10)), target: Some(EntityId(1)), ..Default::default() });
        assert_eq!(survival_attacker(&Snapshot::from_state(&state)), None);
    }
}
