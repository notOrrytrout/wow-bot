use crate::maintenance::MaintenanceDecision;
use std::collections::BTreeMap;
use std::time::Instant;
use wow_domain::WorldPosition;
use wow_infra::world_knowledge::AzerothCoreCatalog;
use wow_state::Snapshot;

/// Supported primary skill lines. These IDs come from the WotLK SkillLine table.
pub const PRIMARY_SKILLS: &[u32] = &[164, 165, 171, 182, 186, 197, 202, 333, 393, 755, 773];
pub const COOKING: u32 = 185;
pub const FIRST_AID: u32 = 129;
pub const FISHING: u32 = 356;

/// Revalidate conditions that make optional profession service safe at dispatch
/// time as well as during policy selection.
pub fn training_state_safe(state: &Snapshot) -> bool {
    if state.state.group.lifecycle != wow_state::group::GroupLifecycle::Solo
        || state.state.control.mover.is_some()
        || state.state.position.moving
        || state.state.transport.attached == Some(true)
        || crate::combat::engagement::survival_attacker(state).is_some()
    {
        return false;
    }
    let Some(position) = state
        .state
        .control
        .active_position(state.state.position.player)
    else {
        return false;
    };
    let Some(player) = state
        .state
        .session
        .character_guid
        .map(wow_domain::EntityId)
        .and_then(|player| state.state.entities.0.get(&player))
    else {
        return false;
    };
    player.health.is_some_and(|(current, _)| current > 0)
        && player.in_combat() != Some(true)
        && !crate::maintenance::nearby_quest_offer(state, position)
}

pub fn learned(state: &Snapshot, skill: u32) -> bool {
    state.state.professions.skills.contains_key(&skill)
}

/// Return preserved primary lines and fill free slots with gathering-compatible
/// profession goals. Unknown profession state must not be treated as empty.
pub fn selected_skills(state: &Snapshot) -> Option<Vec<u32>> {
    let professions = &state.state.professions;
    if !professions.known {
        return None;
    }
    let mut selected = PRIMARY_SKILLS
        .iter()
        .copied()
        .filter(|skill| professions.skill(*skill) > 0)
        .collect::<Vec<_>>();
    let preferred = match state.state.capabilities.class_id {
        Some(1 | 2 | 6) => [186, 164],
        Some(3) => [186, 202],
        Some(4 | 7 | 11) => [393, 165],
        Some(5 | 8 | 9) => [197, 333],
        _ => [182, 171],
    };
    let partner = selected.first().and_then(|skill| match skill {
        164 | 202 | 755 => Some(186),
        165 => Some(393),
        171 | 773 => Some(182),
        186 => Some(164),
        393 => Some(165),
        182 => Some(171),
        197 => Some(333),
        _ => None,
    });
    for skill in partner.into_iter().chain(preferred) {
        if selected.len() >= 2 {
            break;
        }
        if !selected.contains(&skill) {
            selected.push(skill);
        }
    }
    selected.extend([FIRST_AID, COOKING, FISHING]);
    Some(selected)
}

/// Return missing skills in legacy service priority order. First Aid is last so
/// it cannot block a primary profession, Cooking, or Fishing.
pub fn missing_skills(state: &Snapshot) -> Option<Vec<u32>> {
    let mut missing = selected_skills(state)?
        .into_iter()
        .filter(|skill| !learned(state, *skill))
        .collect::<Vec<_>>();
    missing.sort_by_key(|skill| skill_priority(*skill));
    Some(missing)
}

pub fn actionable_missing_skills(state: &Snapshot) -> Option<Vec<u32>> {
    Some(
        missing_skills(state)?
            .into_iter()
            .filter(|skill| *skill != FIRST_AID)
            .collect(),
    )
}

fn skill_priority(skill: u32) -> u8 {
    match skill {
        COOKING => 0,
        FISHING => 1,
        skill if PRIMARY_SKILLS.contains(&skill) => 2,
        FIRST_AID => 3,
        _ => 4,
    }
}

fn skill_order(missing: &[u32], skill: u32) -> usize {
    missing
        .iter()
        .position(|missing| *missing == skill)
        .unwrap_or(usize::MAX)
}

/// Preserve the old level-ten city-training gate, with early Cooking, Mining,
/// or Skinning bootstrap when authoritative skill state is available.
pub fn bootstrap_due(state: &Snapshot) -> bool {
    let level = state
        .state
        .session
        .character_guid
        .map(wow_domain::EntityId)
        .and_then(|player| state.state.entities.0.get(&player))
        .and_then(|player| player.level)
        .unwrap_or_default();
    if !state.state.professions.known {
        return false;
    }
    let Some(missing) = actionable_missing_skills(state) else {
        return false;
    };
    if level >= 10 {
        return !missing.is_empty();
    }
    missing
        .iter()
        .any(|skill| matches!(*skill, COOKING | 186 | 393))
}

pub fn is_nearby_profession_trainer(state: &Snapshot, trainer: wow_domain::EntityId) -> bool {
    let Some(position) = state
        .state
        .control
        .active_position(state.state.position.player)
    else {
        return false;
    };
    let Some(entity) = state.state.entities.0.get(&trainer) else {
        return false;
    };
    let Some(wanted) = actionable_missing_skills(state) else {
        return false;
    };
    if !crate::interaction::is_nearby_interactable_unit(entity, position, 5.0)
        || !entity
            .npc_flags
            .is_some_and(crate::maintenance::is_profession_trainer_flags)
    {
        return false;
    }
    let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
    catalog
        .world()
        .trainer_services
        .iter()
        .find(|service| service.entry_id == entity.entry)
        .is_some_and(|service| service.skills.iter().any(|skill| wanted.contains(skill)))
}

/// Check a live offer against the trusted trainer spell-to-skill mapping and
/// selected missing profession goals. The current trainer list remains primary.
pub fn offer_skill(state: &Snapshot, trainer: wow_domain::EntityId, spell: u32) -> Option<u32> {
    if !bootstrap_due(state)
        || !is_nearby_profession_trainer(state, trainer)
        || state.state.trainer.trainer != Some(trainer)
    {
        return None;
    }
    let entity = state.state.entities.0.get(&trainer)?;
    let skill = wow_infra::world_knowledge::embedded_azerothcore_catalog()
        .profession_skill_for_trainer_spell(entity.entry, spell)?;
    actionable_missing_skills(state)?
        .contains(&skill)
        .then_some(skill)
}

pub fn training_decision(
    state: &Snapshot,
    trainer: wow_domain::EntityId,
    retry_after: &BTreeMap<(u32, wow_domain::EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    if !bootstrap_due(state)
        || !training_state_safe(state)
        || !is_nearby_profession_trainer(state, trainer)
    {
        return None;
    }
    if state.state.trainer.trainer != Some(trainer) {
        return (!retry_after.get(&(0, trainer)).is_some_and(|at| *at > now))
            .then_some(MaintenanceDecision::TrainerList { trainer });
    }
    if state.state.trainer.trainer_type != Some(2)
        || retry_after
            .get(&(u32::MAX, trainer))
            .is_some_and(|at| *at > now)
    {
        return None;
    }
    let player_level = state
        .state
        .session
        .character_guid
        .map(wow_domain::EntityId)
        .and_then(|player| state.state.entities.0.get(&player))
        .and_then(|player| player.level)?;
    let missing = actionable_missing_skills(state)?;
    let mut offers = state
        .state
        .trainer
        .offers
        .iter()
        .filter(|offer| {
            offer_skill(state, trainer, offer.spell).is_some()
                && offer.usable == 0
                && offer.required_level as u32 <= player_level
                && !state.state.capabilities.spells.contains(&offer.spell)
                && (offer.required_skill_line == 0
                    || (state.state.professions.known
                        && u32::from(state.state.professions.skill(offer.required_skill_line))
                            >= offer.required_skill_rank))
                && u64::from(offer.cost_copper)
                    .saturating_add(wow_domain::MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER)
                    <= state.state.inventory.money
                && !retry_after
                    .get(&(offer.spell, trainer))
                    .is_some_and(|at| *at > now)
        })
        .collect::<Vec<_>>();
    offers.sort_by_key(|offer| {
        (
            offer_skill(state, trainer, offer.spell)
                .map(|skill| skill_order(&missing, skill))
                .unwrap_or(usize::MAX),
            std::cmp::Reverse(offer.required_level),
            offer.cost_copper,
            offer.spell,
        )
    });
    offers.first().map(|offer| MaintenanceDecision::TrainerBuy {
        trainer,
        spell: offer.spell,
    })
}

/// Check each live nearby trainer in distance order. A current but stale or
/// irrelevant trainer list does not prevent a query to another nearby trainer.
pub fn nearby_training_decision(
    state: &Snapshot,
    retry_after: &BTreeMap<(u32, wow_domain::EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    let mut trainers = state
        .state
        .entities
        .0
        .values()
        .filter(|entity| is_nearby_profession_trainer(state, entity.id))
        .map(|entity| entity.id)
        .collect::<Vec<_>>();
    let position = state
        .state
        .control
        .active_position(state.state.position.player);
    trainers.sort_by(|left, right| {
        let distance = |trainer: &wow_domain::EntityId| {
            position
                .and_then(|player| {
                    state
                        .state
                        .entities
                        .0
                        .get(trainer)?
                        .position
                        .map(|target| (player, target))
                })
                .map(|(player, target)| player.point.distance(target.point))
                .unwrap_or(f32::INFINITY)
        };
        distance(left).total_cmp(&distance(right))
    });
    if let Some(current) = state.state.trainer.trainer
        && let Some(index) = trainers.iter().position(|trainer| *trainer == current)
    {
        trainers.swap(0, index);
    }
    for trainer in trainers {
        if let Some(decision) = training_decision(state, trainer, retry_after, now) {
            return Some(decision);
        }
    }
    None
}

/// Return a location for a trainer for an unlearned selected profession.
/// The result is advisory: movement and interaction still require normal action
/// validation and live target authority.
pub fn trainer_destination(
    state: &Snapshot,
    catalog: &AzerothCoreCatalog,
) -> Option<(u32, WorldPosition)> {
    if !state.state.professions.known {
        return None;
    }
    let pos = state.state.position.player?;
    if !bootstrap_due(state) {
        return None;
    }
    let (entry, point) = actionable_missing_skills(state)?
        .into_iter()
        .filter(|skill| !learned(state, *skill))
        .find_map(|skill| catalog.nearest_profession_trainer(skill, pos.map, pos.point))?;
    Some((
        entry,
        WorldPosition {
            map: pos.map,
            point,
            orientation: 0.0,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, Snapshot};

    #[test]
    fn profession_selection_requires_known_ranks_and_preserves_known_lines() {
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        assert_eq!(selected_skills(&state), None);
        state.state.professions.known = true;
        state.state.professions.skills.insert(164, (75, 150));
        assert_eq!(selected_skills(&state).unwrap()[..2], [164, 186]);
    }

    #[test]
    fn class_preferences_and_legacy_missing_priority_are_preserved() {
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        state.state.professions.known = true;
        state.state.capabilities.class_id = Some(3);
        assert_eq!(selected_skills(&state).unwrap()[..2], [186, 202]);
        assert_eq!(
            missing_skills(&state).unwrap()[..3],
            [COOKING, FISHING, 186]
        );
        state.state.professions.skills.insert(COOKING, (1, 75));
        assert_eq!(missing_skills(&state).unwrap()[0], FISHING);
    }

    #[test]
    fn first_aid_is_last_and_does_not_block_early_bootstrap() {
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        state.state.session.character_guid = Some(7);
        state.state.entities.0.insert(
            wow_domain::EntityId(7),
            wow_state::entities::EntityState {
                id: wow_domain::EntityId(7),
                level: Some(5),
                ..Default::default()
            },
        );
        state.state.professions.known = true;
        assert!(bootstrap_due(&state));
        assert_eq!(missing_skills(&state).unwrap().last(), Some(&FIRST_AID));
        state.state.professions.skills.insert(COOKING, (1, 75));
        state.state.professions.skills.insert(186, (1, 75));
        state.state.professions.skills.insert(393, (1, 75));
        assert!(!bootstrap_due(&state));
    }

    #[test]
    fn first_aid_only_does_not_start_service_or_travel() {
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        state.state.professions.known = true;
        state.state.capabilities.class_id = Some(1);
        state.state.session.character_guid = Some(7);
        state.state.position.player = Some(WorldPosition {
            map: 0,
            point: wow_domain::Vec3::default(),
            orientation: 0.0,
        });
        state.state.entities.0.insert(
            wow_domain::EntityId(7),
            wow_state::entities::EntityState {
                id: wow_domain::EntityId(7),
                level: Some(20),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        for skill in [164, 186, COOKING, FISHING] {
            state.state.professions.skills.insert(skill, (1, 75));
        }
        assert_eq!(missing_skills(&state).unwrap(), vec![FIRST_AID]);
        assert!(actionable_missing_skills(&state).unwrap().is_empty());
        assert!(!bootstrap_due(&state));
        assert!(
            trainer_destination(
                &state,
                wow_infra::world_knowledge::embedded_azerothcore_catalog()
            )
            .is_none()
        );
    }

    #[test]
    fn nearby_training_checks_an_alternate_trainer_after_irrelevant_list() {
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let trainers = catalog
            .world()
            .trainer_services
            .iter()
            .filter(|service| service.skills.contains(&COOKING))
            .take(2)
            .collect::<Vec<_>>();
        assert_eq!(trainers.len(), 2);
        let player = wow_domain::EntityId(1);
        let first = wow_domain::EntityId(2);
        let second = wow_domain::EntityId(3);
        let position = WorldPosition {
            map: 0,
            point: wow_domain::Vec3::default(),
            orientation: 0.0,
        };
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        state.state.professions.known = true;
        state.state.session.in_world = true;
        state.state.session.character_guid = Some(player.0);
        state.state.position.player = Some(position);
        state.state.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                level: Some(20),
                health: Some((100, 100)),
                position: Some(position),
                ..Default::default()
            },
        );
        for (id, service) in [(first, trainers[0]), (second, trainers[1])] {
            state.state.entities.0.insert(
                id,
                wow_state::entities::EntityState {
                    id,
                    entry: service.entry_id,
                    kind: wow_state::entities::EntityKind::Unit,
                    interactable: true,
                    npc_flags: Some(0x50),
                    position: Some(position),
                    ..Default::default()
                },
            );
        }
        state.state.trainer.trainer = Some(first);
        state.state.trainer.trainer_type = Some(2);
        state.state.trainer.offers = vec![wow_state::trainer::TrainerSpellOffer {
            spell: u32::MAX,
            usable: 0,
            ..Default::default()
        }];
        assert_eq!(
            nearby_training_decision(&state, &BTreeMap::new(), Instant::now()),
            Some(MaintenanceDecision::TrainerList { trainer: second })
        );
    }

    #[test]
    fn profession_offer_order_follows_cooking_fishing_primary_then_first_aid() {
        let missing = vec![COOKING, FISHING, 186, FIRST_AID];
        let mut skills = [FIRST_AID, 186, FISHING, COOKING];
        skills.sort_by_key(|skill| skill_order(&missing, *skill));
        assert_eq!(skills, [COOKING, FISHING, 186, FIRST_AID]);
    }

    #[test]
    fn trainer_is_unavailable_without_known_ranks_position_or_matching_catalog() {
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        state.state.professions.known = true;
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        assert!(trainer_destination(&state, catalog).is_none());
        state.state.position.player = Some(WorldPosition {
            map: 0,
            point: wow_domain::Vec3::default(),
            orientation: 0.0,
        });
        assert!(trainer_destination(&state, catalog).is_some());
    }

    #[test]
    fn profession_training_selects_only_current_mapped_missing_offer() {
        let catalog = wow_infra::world_knowledge::embedded_azerothcore_catalog();
        let service = catalog
            .world()
            .trainer_services
            .iter()
            .find(|service| {
                service
                    .spell_skills
                    .iter()
                    .any(|mapping| mapping.skill == COOKING)
            })
            .expect("cooking trainer mapping");
        let spell = service
            .spell_skills
            .iter()
            .find(|mapping| mapping.skill == COOKING)
            .unwrap()
            .spell;
        let trainer = wow_domain::EntityId(2);
        let player = wow_domain::EntityId(1);
        let position = WorldPosition {
            map: 0,
            point: wow_domain::Vec3::default(),
            orientation: 0.0,
        };
        let mut state = Snapshot::from_state(&AuthoritativeState::default());
        state.state.professions.known = true;
        state.state.position.player = Some(position);
        state.state.session.in_world = true;
        state.state.session.character_guid = Some(player.0);
        state.state.inventory.money = 2_000;
        state.state.entities.0.insert(
            player,
            wow_state::entities::EntityState {
                id: player,
                level: Some(20),
                health: Some((100, 100)),
                position: Some(position),
                ..Default::default()
            },
        );
        state.state.entities.0.insert(
            trainer,
            wow_state::entities::EntityState {
                id: trainer,
                entry: service.entry_id,
                kind: wow_state::entities::EntityKind::Unit,
                interactable: true,
                npc_flags: Some(0x50),
                position: Some(position),
                ..Default::default()
            },
        );
        assert_eq!(
            training_decision(&state, trainer, &BTreeMap::new(), Instant::now()),
            Some(MaintenanceDecision::TrainerList { trainer })
        );
        state.state.trainer.trainer = Some(trainer);
        state.state.trainer.trainer_type = Some(2);
        state.state.trainer.offers = vec![wow_state::trainer::TrainerSpellOffer {
            spell,
            usable: 0,
            cost_copper: 500,
            required_level: 1,
            ..Default::default()
        }];
        assert_eq!(
            training_decision(&state, trainer, &BTreeMap::new(), Instant::now()),
            Some(MaintenanceDecision::TrainerBuy { trainer, spell })
        );
        state.state.trainer.offers[0].spell = u32::MAX;
        assert_eq!(
            training_decision(&state, trainer, &BTreeMap::new(), Instant::now()),
            None
        );
    }
}
