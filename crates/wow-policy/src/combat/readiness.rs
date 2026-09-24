use wow_domain::EntityId;
use wow_state::{Snapshot, entities::EntityState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpellUnavailableReason {
    Unknown,
    UnknownMetadata,
    UnknownState,
    WrongClass,
    Cooldown,
    GlobalCooldown,
    InsufficientPower,
    InsufficientRunes,
    MissingComboPoints,
    MissingReagent,
    MissingEquipment,
    RequirementNotMet,
    UnsupportedRequirement,
    TargetNotAuthoritative,
    NotInWorld,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpellUsePermit;

/// One fail-closed gate for class, cost, cooldown, resource, target, and DBC
/// requirements. Range and final command legality remain in engine validation.
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
    let metadata =
        crate::combat::spells::metadata(spell).ok_or(SpellUnavailableReason::UnknownMetadata)?;
    if snapshot
        .state
        .capabilities
        .spell_cooldowns
        .get(&spell)
        .is_some_and(|ready_at| *ready_at > now_ms)
    {
        return Err(SpellUnavailableReason::Cooldown);
    }
    if let (Some(source), Some(started)) = (
        snapshot.state.capabilities.global_cooldown_spell,
        snapshot.state.capabilities.global_cooldown_started_at_ms,
    ) && let Some(source_spell) = crate::combat::spells::metadata(source)
        && source_spell.global_cooldown.category != 0
        && started.saturating_add(u64::from(source_spell.global_cooldown.duration_ms)) > now_ms
    {
        return Err(SpellUnavailableReason::GlobalCooldown);
    }

    let player_id = snapshot
        .state
        .session
        .character_guid
        .map(EntityId)
        .ok_or(SpellUnavailableReason::UnknownState)?;
    let player = snapshot
        .state
        .entities
        .0
        .get(&player_id)
        .ok_or(SpellUnavailableReason::UnknownState)?;
    if !metadata.classes.contains(
        &snapshot
            .state
            .capabilities
            .class_id
            .ok_or(SpellUnavailableReason::UnknownState)?,
    ) {
        return Err(SpellUnavailableReason::WrongClass);
    }
    if let Some(target) = target {
        if target != player_id && !snapshot.state.entities.0.contains_key(&target) {
            return Err(SpellUnavailableReason::TargetNotAuthoritative);
        }
    } else if metadata.requirements.targets != 0 {
        return Err(SpellUnavailableReason::TargetNotAuthoritative);
    }

    check_power(snapshot, player_id, player, metadata)?;
    check_runes(snapshot, metadata)?;
    if metadata.requires_combo_points {
        let target = target.ok_or(SpellUnavailableReason::UnknownState)?;
        let points = snapshot
            .state
            .capabilities
            .combo_points
            .get(&target)
            .ok_or(SpellUnavailableReason::UnknownState)?;
        if *points == 0 {
            return Err(SpellUnavailableReason::MissingComboPoints);
        }
    }
    check_reagents(snapshot, metadata)?;
    check_equipment(snapshot, metadata)?;
    check_form(player, metadata)?;
    check_aura_requirements(snapshot, player_id, target, metadata)?;
    if metadata.requirements.spell_focus != 0 || metadata.requirements.target_creature_type != 0 {
        return Err(SpellUnavailableReason::UnsupportedRequirement);
    }
    Ok(SpellUsePermit)
}

fn check_power(
    snapshot: &Snapshot,
    player_id: EntityId,
    player: &EntityState,
    spell: &crate::combat::spells::SpellMetadata,
) -> Result<(), SpellUnavailableReason> {
    let cost = &spell.cost;
    let has_cost = cost.base != 0
        || cost.percent != 0
        || cost.per_second != 0
        || cost.per_second_per_level != 0
        || cost.use_all_power;
    if !has_cost {
        return Ok(());
    }
    if has_matching_spell_cost_modifier(snapshot, player_id, spell) {
        return Err(SpellUnavailableReason::UnknownState);
    }
    if spell.power_type == 5 {
        return Ok(());
    }

    let (current, percent_base) = match spell.power_type {
        -2 => (
            player.health.ok_or(SpellUnavailableReason::UnknownState)?.0,
            player
                .base_health
                .ok_or(SpellUnavailableReason::UnknownState)?,
        ),
        0 => (
            power_of_type(player, 0)?,
            if cost.percent != 0 {
                player
                    .base_mana
                    .ok_or(SpellUnavailableReason::UnknownState)?
            } else {
                0
            },
        ),
        1..=4 | 6 => {
            let (_, maximum) = power_pair_of_type(player, spell.power_type as u8)?;
            let current = power_of_type(player, spell.power_type as u8)?;
            (current, if cost.percent != 0 { maximum } else { 0 })
        }
        _ => return Err(SpellUnavailableReason::UnsupportedRequirement),
    };
    if cost.use_all_power {
        return (current > 0)
            .then_some(())
            .ok_or(SpellUnavailableReason::InsufficientPower);
    }

    let mut amount = i64::from(cost.base);
    if cost.percent != 0 {
        amount += i64::from(percent_base) * i64::from(cost.percent) / 100;
    }
    if cost.per_second != 0 || cost.per_second_per_level != 0 {
        let duration = spell
            .duration_ms
            .filter(|duration| *duration >= 0)
            .ok_or(SpellUnavailableReason::UnknownState)?;
        let ticks = u64::try_from(duration).unwrap_or_default().div_ceil(1000);
        let per_tick = u64::from(cost.per_second)
            + u64::from(cost.per_second_per_level)
                * u64::from(player.level.ok_or(SpellUnavailableReason::UnknownState)?);
        amount += i64::try_from(ticks.saturating_mul(per_tick)).unwrap_or(i64::MAX);
    }

    let school = spell
        .school_mask
        .trailing_zeros()
        .try_into()
        .ok()
        .filter(|index: &usize| *index < 7)
        .ok_or(SpellUnavailableReason::UnknownMetadata)?;
    let flat_modifiers = player
        .power_cost_modifiers
        .ok_or(SpellUnavailableReason::UnknownState)?;
    let multipliers = player
        .power_cost_multipliers
        .ok_or(SpellUnavailableReason::UnknownState)?;
    let multiplier = multipliers[school];
    if !multiplier.is_finite() {
        return Err(SpellUnavailableReason::UnknownState);
    }
    amount += i64::from(flat_modifiers[school]);

    if spell.requirements.attributes_ex4 & 0x00000400 != 0 {
        let speed_ms = if let Some(form) = player.shapeshift_form.filter(|form| *form != 0) {
            crate::combat::spells::shapeshift_form_metadata(form)
                .map(|form| form.attack_speed_ms)
                .ok_or(SpellUnavailableReason::UnknownMetadata)?
        } else {
            let attack_times = player
                .base_attack_time_ms
                .ok_or(SpellUnavailableReason::UnknownState)?;
            if spell.requirements.attributes_ex3 & 0x01000000 != 0 {
                attack_times[1]
            } else {
                attack_times[0]
            }
        };
        amount += i64::from(speed_ms / 100);
    }

    amount = ((amount as f64) * (1.0 + f64::from(multiplier))).trunc() as i64;
    amount = amount.max(0);
    if spell.power_type == -2 {
        if current <= u32::try_from(amount).unwrap_or(u32::MAX) {
            return Err(SpellUnavailableReason::InsufficientPower);
        }
    } else if current < u32::try_from(amount).unwrap_or(u32::MAX) {
        return Err(SpellUnavailableReason::InsufficientPower);
    }
    Ok(())
}

fn power_pair_of_type(
    player: &EntityState,
    power_type: u8,
) -> Result<(u32, u32), SpellUnavailableReason> {
    if player.power_type == Some(power_type) {
        return player.power.ok_or(SpellUnavailableReason::UnknownState);
    }
    Err(SpellUnavailableReason::UnknownState)
}

fn power_of_type(player: &EntityState, power_type: u8) -> Result<u32, SpellUnavailableReason> {
    Ok(power_pair_of_type(player, power_type)?.0)
}

fn has_matching_spell_cost_modifier(
    snapshot: &Snapshot,
    player: EntityId,
    spell: &crate::combat::spells::SpellMetadata,
) -> bool {
    snapshot
        .state
        .auras
        .spells(player)
        .iter()
        .filter_map(|aura| crate::combat::spells::metadata(*aura))
        .flat_map(|aura| &aura.cost_spell_modifiers)
        .any(|modifier| {
            modifier.family_flags.iter().all(|flags| *flags == 0)
                || modifier
                    .family_flags
                    .iter()
                    .zip(spell.family_flags)
                    .any(|(modifier, spell)| modifier & spell != 0)
        })
}

fn check_runes(
    snapshot: &Snapshot,
    spell: &crate::combat::spells::SpellMetadata,
) -> Result<(), SpellUnavailableReason> {
    let Some(cost) = spell.runes else {
        return Ok(());
    };
    let required = [cost.blood, cost.frost, cost.unholy];
    if required.iter().all(|count| *count == 0) {
        return Ok(());
    }
    let runes = snapshot
        .state
        .capabilities
        .runes
        .as_ref()
        .ok_or(SpellUnavailableReason::UnknownState)?;
    if runes.len() != 6 {
        return Err(SpellUnavailableReason::UnknownState);
    }
    let mut available = [0u32; 4];
    for rune in runes.iter().filter(|rune| rune.ready) {
        let rune_type = usize::from(rune.rune_type);
        if rune_type >= available.len() {
            return Err(SpellUnavailableReason::UnknownState);
        }
        available[rune_type] += 1;
    }
    let mut death_shortfall = 0;
    for (rune_type, required) in required.into_iter().enumerate() {
        let supplied = available[rune_type].min(required);
        available[rune_type] -= supplied;
        death_shortfall += required - supplied;
    }
    if available[3] < death_shortfall {
        return Err(SpellUnavailableReason::InsufficientRunes);
    }
    Ok(())
}

fn check_reagents(
    snapshot: &Snapshot,
    spell: &crate::combat::spells::SpellMetadata,
) -> Result<(), SpellUnavailableReason> {
    for reagent in &spell.reagents {
        let item = u32::try_from(reagent.item)
            .map_err(|_| SpellUnavailableReason::UnsupportedRequirement)?;
        if !snapshot.state.inventory.has(item, reagent.count) {
            return Err(SpellUnavailableReason::MissingReagent);
        }
    }
    Ok(())
}

fn check_equipment(
    snapshot: &Snapshot,
    spell: &crate::combat::spells::SpellMetadata,
) -> Result<(), SpellUnavailableReason> {
    let required = spell.equipment;
    if required.class_id < 0 {
        return Ok(());
    }
    if !snapshot.state.inventory.equipment_slots_authoritative {
        return Err(SpellUnavailableReason::UnknownState);
    }
    let usable = snapshot
        .state
        .inventory
        .equipped_items
        .values()
        .any(|item| {
            let Some(item) = crate::combat::spells::item_metadata(*item) else {
                return false;
            };
            if item.class_id != required.class_id as u32 {
                return false;
            }
            if required.subclass_mask > 0
                && required.subclass_mask as u32 & (1u32.checked_shl(item.subclass).unwrap_or(0))
                    == 0
            {
                return false;
            }
            required.inventory_mask <= 0
                || required.inventory_mask as u32
                    & (1u32.checked_shl(item.inventory_type).unwrap_or(0))
                    != 0
        });
    if usable {
        Ok(())
    } else {
        Err(SpellUnavailableReason::MissingEquipment)
    }
}

fn check_form(
    player: &EntityState,
    spell: &crate::combat::spells::SpellMetadata,
) -> Result<(), SpellUnavailableReason> {
    let req = &spell.requirements;
    let not_shapeshifted = req.attributes & 0x00010000 != 0;
    let can_cast_unshifted = req.attributes_ex2 & 0x00080000 != 0;
    if req.stance_mask == 0 && req.stance_exclude == 0 && !not_shapeshifted {
        return Ok(());
    }
    let form = player
        .shapeshift_form
        .ok_or(SpellUnavailableReason::UnknownState)?;
    let form_mask = if (1..=32).contains(&form) {
        1u32 << (form - 1)
    } else {
        0
    };
    if form_mask & req.stance_exclude != 0
        || (form != 0 && not_shapeshifted && form_mask & req.stance_mask == 0)
        || (form == 0 && req.stance_mask != 0 && !can_cast_unshifted)
        || (form != 0
            && req.stance_mask != 0
            && form_mask & req.stance_mask == 0
            && !can_cast_unshifted)
    {
        return Err(SpellUnavailableReason::RequirementNotMet);
    }
    if form != 0
        && crate::combat::spells::shapeshift_form_metadata(form)
            .ok_or(SpellUnavailableReason::UnknownMetadata)?
            .flags
            & 0x00000400
            != 0
        && form_mask & req.stance_mask == 0
    {
        return Err(SpellUnavailableReason::RequirementNotMet);
    }
    Ok(())
}

fn check_aura_requirements(
    snapshot: &Snapshot,
    player: EntityId,
    target: Option<EntityId>,
    spell: &crate::combat::spells::SpellMetadata,
) -> Result<(), SpellUnavailableReason> {
    let requirements = &spell.requirements;
    let player_state = snapshot
        .state
        .entities
        .0
        .get(&player)
        .ok_or(SpellUnavailableReason::UnknownState)?;
    if requirements.caster_aura_state != 0 || requirements.caster_aura_state_not != 0 {
        let state = player_state
            .aura_state
            .ok_or(SpellUnavailableReason::UnknownState)?;
        if state & requirements.caster_aura_state != requirements.caster_aura_state
            || state & requirements.caster_aura_state_not != 0
        {
            return Err(SpellUnavailableReason::RequirementNotMet);
        }
    }
    let player_auras = snapshot.state.auras.spells(player);
    if (requirements.caster_aura_spell != 0
        && !player_auras.contains(&requirements.caster_aura_spell))
        || (requirements.exclude_caster_aura_spell != 0
            && player_auras.contains(&requirements.exclude_caster_aura_spell))
    {
        return Err(SpellUnavailableReason::RequirementNotMet);
    }
    if requirements.target_aura_state != 0
        || requirements.target_aura_state_not != 0
        || requirements.target_aura_spell != 0
        || requirements.exclude_target_aura_spell != 0
    {
        let target = target.ok_or(SpellUnavailableReason::UnknownState)?;
        let target_state = snapshot
            .state
            .entities
            .0
            .get(&target)
            .ok_or(SpellUnavailableReason::UnknownState)?;
        if requirements.target_aura_state != 0 || requirements.target_aura_state_not != 0 {
            let state = target_state
                .aura_state
                .ok_or(SpellUnavailableReason::UnknownState)?;
            if state & requirements.target_aura_state != requirements.target_aura_state
                || state & requirements.target_aura_state_not != 0
            {
                return Err(SpellUnavailableReason::RequirementNotMet);
            }
        }
        let target_auras = snapshot.state.auras.spells(target);
        if (requirements.target_aura_spell != 0
            && !target_auras.contains(&requirements.target_aura_spell))
            || (requirements.exclude_target_aura_spell != 0
                && target_auras.contains(&requirements.exclude_target_aura_spell))
        {
            return Err(SpellUnavailableReason::RequirementNotMet);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::WorldPosition;
    use wow_state::{
        AuthoritativeState,
        capabilities::RuneState,
        entities::{EntityKind, EntityState},
    };

    fn make_state(spell: u32, class: u8, power_type: u8, power: u32) -> AuthoritativeState {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.capabilities.class_id = Some(class);
        state.capabilities.spells.insert(spell);
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                power_type: Some(power_type),
                power: Some((power, 100)),
                health: Some((100, 100)),
                base_health: Some(100),
                base_mana: Some(100),
                power_cost_modifiers: Some([0; 7]),
                power_cost_multipliers: Some([0.0; 7]),
                shapeshift_form: Some(0),
                aura_state: Some(0),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                kind: EntityKind::Unit,
                position: Some(WorldPosition::default()),
                aura_state: Some(0),
                ..Default::default()
            },
        );
        state
    }

    #[test]
    fn catalog_contains_spell_costs_and_requirements_for_every_class() {
        for class in [1, 2, 3, 4, 5, 6, 7, 8, 9, 11] {
            assert!(
                crate::combat::spells::metadata(686).is_some_and(|spell| !spell.classes.is_empty())
            );
            assert!(
                crate::combat::selector::supported_class_trees()
                    .any(|(policy_class, _, _)| policy_class == class)
            );
        }
        assert!(
            crate::combat::spells::metadata(45477)
                .is_some_and(|spell| spell.runes.is_some_and(|runes| runes.unholy != 0))
        );
        assert!(
            crate::combat::spells::metadata(48577).is_some_and(|spell| spell.requires_combo_points)
        );
    }

    #[test]
    fn costs_cooldowns_and_unknown_required_state_fail_closed() {
        let mut state = make_state(686, 9, 0, 100);
        let mut snapshot = Snapshot::from_state(&state);
        assert!(check_spell_readiness(&snapshot, 686, Some(EntityId(9)), u64::MAX).is_ok());
        state.entities.0.get_mut(&EntityId(1)).unwrap().base_mana = Some(1000);
        state.entities.0.get_mut(&EntityId(1)).unwrap().power = Some((0, 100));
        assert_eq!(
            check_spell_readiness(
                &Snapshot::from_state(&state),
                686,
                Some(EntityId(9)),
                u64::MAX
            ),
            Err(SpellUnavailableReason::InsufficientPower)
        );
        state.entities.0.get_mut(&EntityId(1)).unwrap().power = Some((100, 100));
        state.capabilities.spell_cooldowns.insert(686, u64::MAX);
        assert_eq!(
            check_spell_readiness(&Snapshot::from_state(&state), 686, Some(EntityId(9)), 1),
            Err(SpellUnavailableReason::Cooldown)
        );
        state.capabilities.spell_cooldowns.clear();
        state.capabilities.runes = Some(vec![RuneState {
            rune_type: 0,
            ready: true,
        }]);
        snapshot = Snapshot::from_state(&state);
        assert_eq!(
            check_spell_readiness(&snapshot, 686, Some(EntityId(77)), 1),
            Err(SpellUnavailableReason::TargetNotAuthoritative)
        );
    }

    #[test]
    fn rune_combo_form_reagent_and_equipment_gates_use_authoritative_state() {
        let dk_spell = 45477;
        let mut state = make_state(dk_spell, 6, 6, 100);
        let mut snap = Snapshot::from_state(&state);
        assert_eq!(
            check_spell_readiness(&snap, dk_spell, Some(EntityId(9)), u64::MAX),
            Err(SpellUnavailableReason::UnknownState)
        );
        state.capabilities.runes = Some(vec![
            RuneState {
                rune_type: 0,
                ready: true,
            },
            RuneState {
                rune_type: 0,
                ready: true,
            },
            RuneState {
                rune_type: 0,
                ready: true,
            },
            RuneState {
                rune_type: 1,
                ready: true,
            },
            RuneState {
                rune_type: 2,
                ready: true,
            },
            RuneState {
                rune_type: 1,
                ready: true,
            },
        ]);
        snap = Snapshot::from_state(&state);
        assert!(check_spell_readiness(&snap, dk_spell, Some(EntityId(9)), u64::MAX).is_ok());

        let finisher = 48577;
        state = make_state(finisher, 11, 3, 100);
        state
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .shapeshift_form = Some(1);
        assert_eq!(
            check_spell_readiness(
                &Snapshot::from_state(&state),
                finisher,
                Some(EntityId(9)),
                u64::MAX
            ),
            Err(SpellUnavailableReason::UnknownState)
        );
        state.capabilities.combo_points.insert(EntityId(9), 0);
        assert_eq!(
            check_spell_readiness(
                &Snapshot::from_state(&state),
                finisher,
                Some(EntityId(9)),
                u64::MAX
            ),
            Err(SpellUnavailableReason::MissingComboPoints)
        );
        state.capabilities.combo_points.insert(EntityId(9), 2);
        assert!(
            check_spell_readiness(
                &Snapshot::from_state(&state),
                finisher,
                Some(EntityId(9)),
                u64::MAX
            )
            .is_ok()
        );
    }
}
