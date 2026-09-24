use wow_domain::EntityId;
use wow_state::Snapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpellUnavailableReason {
    Unknown,
    Cooldown,
    InsufficientPower,
    TargetNotAuthoritative,
    NotInWorld,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpellUsePermit;

/// Shared deterministic spell-readiness gate used by combat, maintenance and
/// controlled quest spell work. Target geometry remains in final action
/// validation; this gate owns capability/cooldown/resource authority.
pub fn check_spell_readiness(
    snapshot: &Snapshot,
    spell: u32,
    target: Option<EntityId>,
    now_ms: u64,
) -> Result<SpellUsePermit, SpellUnavailableReason> {
    if !snapshot.state.session.in_world {
        return Err(SpellUnavailableReason::NotInWorld);
    }
    if !snapshot.state.capabilities.spells.contains(&spell)
        && !snapshot.state.control.abilities.contains(&spell)
    {
        return Err(SpellUnavailableReason::Unknown);
    }
    if snapshot
        .state
        .capabilities
        .spell_cooldowns
        .get(&spell)
        .is_some_and(|ready_at| *ready_at > now_ms)
    {
        return Err(SpellUnavailableReason::Cooldown);
    }
    if let Some(target) = target {
        let self_guid = snapshot.state.session.character_guid.map(EntityId);
        if self_guid != Some(target) && !snapshot.state.entities.0.contains_key(&target) {
            return Err(SpellUnavailableReason::TargetNotAuthoritative);
        }
    }
    // Current state exposes mana authoritatively for mana users. Until richer
    // Spell.dbc costs are projected, fail closed only at zero mana rather than
    // duplicating class-specific cost estimates in callers.
    if let Some(player) = snapshot.state.session.character_guid.map(EntityId)
        && snapshot
            .state
            .entities
            .0
            .get(&player)
            .filter(|entity| entity.power_type == Some(0))
            .and_then(|entity| entity.power)
            .is_some_and(|(current, _)| current == 0)
    {
        return Err(SpellUnavailableReason::InsufficientPower);
    }
    Ok(SpellUsePermit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, entities::EntityState};
    #[test]
    fn cooldown_and_zero_mana_are_shared_fail_closed_reasons() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.capabilities.spells.insert(686);
        state.capabilities.spell_cooldowns.insert(686, 5000);
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                power_type: Some(0),
                power: Some((100, 100)),
                ..Default::default()
            },
        );
        let snap = Snapshot::from_state(&state);
        assert_eq!(
            check_spell_readiness(&snap, 686, None, 1000),
            Err(SpellUnavailableReason::Cooldown)
        );
        state.capabilities.spell_cooldowns.clear();
        state.entities.0.get_mut(&EntityId(1)).unwrap().power = Some((0, 100));
        assert_eq!(
            check_spell_readiness(&Snapshot::from_state(&state), 686, None, 1000),
            Err(SpellUnavailableReason::InsufficientPower)
        );
    }
}
