use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};
use wow_domain::{EntityId, GameplayCommand, time::Millis};
use wow_state::Snapshot;

const CAST_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
const DEFAULT_COMBAT_CYCLE: std::time::Duration = std::time::Duration::from_secs(3);
const SLOW_FALLBACK_CYCLE: std::time::Duration = std::time::Duration::from_secs(12);

#[derive(Clone, Debug, Deserialize)]
struct CombatCatalog {
    format_version: u32,
    classes: BTreeMap<u8, ClassPolicy>,
}

#[derive(Clone, Debug, Deserialize)]
struct ClassPolicy {
    trees: BTreeMap<u8, TreePolicy>,
}

#[derive(Clone, Debug, Deserialize)]
struct TreePolicy {
    power_policies: BTreeMap<u8, Vec<u32>>,
}

#[derive(Clone, Debug, Deserialize)]
struct WandCatalog {
    wand_items: Vec<u32>,
}

static COMBAT: OnceLock<CombatCatalog> = OnceLock::new();
static WANDS: OnceLock<WandCatalog> = OnceLock::new();

fn combat_catalog() -> &'static CombatCatalog {
    COMBAT.get_or_init(|| {
        let catalog: CombatCatalog =
            serde_json::from_str(include_str!("../../data/combat-priorities.json"))
                .expect("combat priority catalog");
        assert_eq!(catalog.format_version, 2);
        catalog
    })
}

fn wand_catalog() -> &'static WandCatalog {
    WANDS.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/wands.json")).expect("wand catalog")
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum CombatDecision {
    Cast { spell: u32, target: EntityId },
    Wand { target: EntityId },
    Melee { target: EntityId },
    Deferred { reason: &'static str },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CombatAction {
    pub command: GameplayCommand,
    pub cycle: std::time::Duration,
}

pub fn select(snapshot: &Snapshot, target: EntityId) -> CombatDecision {
    let Some(class_id) = snapshot.state.capabilities.class_id else {
        return deferred("class_not_authoritative");
    };
    let Some(policy) = combat_catalog().classes.get(&class_id) else {
        return deferred("class_specialization_policy_unavailable");
    };
    let tree = snapshot.state.capabilities.specialization_tree;
    if tree.is_none() && !player_is_pre_specialization(snapshot) {
        return deferred("specialization_not_authoritative");
    }
    let Some((power_type, _)) = player_power(snapshot) else {
        return deferred("combat_resource_state_unknown");
    };
    let priorities = match tree {
        Some(tree) => {
            let Some(tree_policy) = policy.trees.get(&tree) else {
                return deferred("class_specialization_policy_unavailable");
            };
            tree_policy.power_policies.get(&power_type).cloned()
        }
        None => {
            // Before level 10, WotLK characters do not have a locked talent
            // tree. Use only spells that the server has confirmed the player
            // knows from the class priorities, following the old policy's
            // class-level behavior when no talent tree is locked.
            let mut priorities = Vec::new();
            for tree_policy in policy.trees.values() {
                if let Some(families) = tree_policy.power_policies.get(&power_type) {
                    for family in families {
                        if !priorities.contains(family) {
                            priorities.push(*family);
                        }
                    }
                }
            }
            (!priorities.is_empty()).then_some(priorities)
        }
    };
    let Some(priorities) = priorities else {
        return deferred("combat_resource_type_mismatch");
    };

    for family_id in priorities {
        let Some(spell) = crate::combat::spells::family_spells(family_id).and_then(|spells| {
            spells
                .iter()
                .find(|spell| snapshot.state.capabilities.spells.contains(spell))
                .copied()
        }) else {
            continue;
        };
        if crate::combat::readiness::check_spell_readiness(
            snapshot,
            spell,
            Some(target),
            Millis::wall_clock_now().0,
        )
        .is_ok()
        {
            return CombatDecision::Cast { spell, target };
        }
    }

    let now_ms = Millis::wall_clock_now().0;
    if let Some(spell) = select_ranged_attack_fallback(snapshot, class_id, target, now_ms) {
        return CombatDecision::Cast { spell, target };
    }
    if is_wand_caster(class_id) && has_equipped_wand(snapshot) {
        return CombatDecision::Wand { target };
    }
    if melee_fallback_allowed(snapshot, class_id, tree) {
        CombatDecision::Melee { target }
    } else {
        deferred("no_safe_offensive_fallback")
    }
}

fn select_ranged_attack_fallback(
    snapshot: &Snapshot,
    class_id: u8,
    target: EntityId,
    now_ms: u64,
) -> Option<u32> {
    const MIN_RANGED_ATTACK_RANGE_YARDS: f32 = 5.5;

    let mut candidates = snapshot
        .state
        .capabilities
        .spells
        .iter()
        .filter_map(|spell| crate::combat::spells::metadata(*spell))
        .filter(|spell| spell.classes.contains(&class_id))
        .filter(|spell| spell.attack_spell)
        .filter(|spell| spell.id != 5019)
        .filter_map(|spell| {
            let (_, maximum) = crate::combat::spells::range_for(spell.id, true)?;
            (maximum > MIN_RANGED_ATTACK_RANGE_YARDS).then_some((spell, maximum))
        })
        .filter(|(spell, _)| {
            crate::combat::readiness::check_spell_readiness(
                snapshot,
                spell.id,
                Some(target),
                now_ms,
            )
            .is_ok()
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|(left, left_max), (right, right_max)| {
        is_basic_ranged_attack(&left.name)
            .cmp(&is_basic_ranged_attack(&right.name))
            .then_with(|| left_max.total_cmp(right_max))
            .then_with(|| spell_rank(&right.rank).cmp(&spell_rank(&left.rank)))
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates.first().map(|(spell, _)| spell.id)
}

fn is_basic_ranged_attack(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "shoot"
        || name.starts_with("shoot ")
        || name == "throw"
        || name.starts_with("throw ")
        || name == "auto shot"
}

fn spell_rank(rank: &str) -> u32 {
    rank.strip_prefix("Rank ")
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

fn melee_fallback_allowed(snapshot: &Snapshot, class_id: u8, tree: Option<u8>) -> bool {
    let player = snapshot.state.session.character_guid.map(EntityId);
    let assigned_tank = player.is_some_and(|player| {
        snapshot.state.group.members.iter().any(|member| {
            member.entity == player && member.role.as_deref().is_some_and(role_is_tank)
        })
    });
    assigned_tank
        || matches!(class_id, 1 | 4 | 6)
        || matches!(
            (class_id, tree),
            (2, Some(2)) | (7, Some(1)) | (11, Some(1))
        )
}

fn role_is_tank(role: &str) -> bool {
    let role = role.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    matches!(role.as_str(), "tank" | "main_tank" | "off_tank")
}

fn player_is_pre_specialization(snapshot: &Snapshot) -> bool {
    snapshot
        .state
        .session
        .character_guid
        .map(EntityId)
        .and_then(|player| snapshot.state.entities.0.get(&player))
        .and_then(|player| player.level)
        .is_some_and(|level| level < 10)
}

/// Convert the policy decision to the one shared action shape used by every engine combat path.
pub fn select_action(snapshot: &Snapshot, target: EntityId) -> Result<CombatAction, &'static str> {
    if let Some(spell) = select_interrupt(snapshot, target) {
        return Ok(cast_action(spell, target, CAST_POLL_INTERVAL));
    }

    if let Some((spell, heal_target)) = select_heal(snapshot) {
        return Ok(cast_action(spell, heal_target, combat_cycle(snapshot)));
    }

    let cycle = combat_cycle(snapshot);
    match select(snapshot, target) {
        CombatDecision::Cast { spell, target } => Ok(cast_action(spell, target, cycle)),
        CombatDecision::Wand { target } => Ok(cast_action(5019, target, fallback_cycle(cycle))),
        CombatDecision::Melee { target } => Ok(CombatAction {
            command: GameplayCommand::Attack(target),
            cycle: fallback_cycle(cycle),
        }),
        CombatDecision::Deferred { reason } => Err(reason),
    }
}

fn cast_action(spell: u32, target: EntityId, cycle: std::time::Duration) -> CombatAction {
    CombatAction {
        command: GameplayCommand::Cast {
            spell,
            target: Some(target),
        },
        cycle,
    }
}

fn fallback_cycle(cycle: std::time::Duration) -> std::time::Duration {
    if cycle < DEFAULT_COMBAT_CYCLE {
        cycle
    } else {
        SLOW_FALLBACK_CYCLE
    }
}

fn select_interrupt(snapshot: &Snapshot, target: EntityId) -> Option<u32> {
    let target_state = snapshot.state.entities.0.get(&target)?;
    if !target_state.hostile || target_state.is_dead() {
        return None;
    }
    let now_ms = Millis::wall_clock_now().0;
    let cast = snapshot.state.active_casts.get(&target)?;
    if cast.ends_at_ms <= now_ms {
        return None;
    }
    let class_id = snapshot.state.capabilities.class_id?;
    let interrupt_ids: &[u32] = match class_id {
        1 => &[6552, 6554, 6555, 12555, 13491, 15615, 19639, 26090, 47081],
        2 => &[31935, 48827, 48826],
        3 => &[34490],
        4 => &[1766, 1767, 1768, 1769, 1770],
        5 => &[],
        6 => &[47528, 53550],
        7 => &[57994],
        8 => &[2139],
        9 => &[],
        11 => &[],
        _ => &[],
    };
    interrupt_ids.iter().copied().find(|spell| {
        snapshot.state.capabilities.spells.contains(spell)
            && crate::combat::readiness::check_spell_readiness(
                snapshot,
                *spell,
                Some(target),
                now_ms,
            )
            .is_ok()
    })
}

fn combat_cycle(snapshot: &Snapshot) -> std::time::Duration {
    let now_ms = Millis::wall_clock_now().0;
    if snapshot
        .state
        .active_casts
        .values()
        .any(|cast| cast.ends_at_ms > now_ms)
    {
        CAST_POLL_INTERVAL
    } else {
        DEFAULT_COMBAT_CYCLE
    }
}

/// Select a known direct heal for the most injured living party member or player.
/// Dedicated healing trees keep a wider reserve; other trees heal only in an
/// emergency. Unknown health and offline group members cannot trigger a cast.
fn select_heal(snapshot: &Snapshot) -> Option<(u32, EntityId)> {
    const HEAL_FAMILIES: &[(u8, &[u32])] = &[
        (2, &[20473, 635, 19750]),  // Holy Paladin
        (5, &[47540, 2061, 2060]),  // Priest
        (7, &[331, 8004, 1064]),    // Shaman
        (11, &[50464, 5185, 8936]), // Druid
    ];

    let class_id = snapshot.state.capabilities.class_id?;
    let tree = snapshot.state.capabilities.specialization_tree;
    let is_healer = matches!(
        (class_id, tree),
        (2, Some(0)) | (5, Some(0 | 1)) | (7, Some(2)) | (11, Some(2))
    );
    let emergency_threshold = if is_healer { 0.65 } else { 0.30 };
    let player = snapshot.state.session.character_guid.map(EntityId)?;

    let mut candidates = vec![player];
    candidates.extend(crate::group::state::online_members_except(
        snapshot,
        Some(player),
    ));

    let target = candidates
        .into_iter()
        .filter_map(|entity| {
            let health = snapshot.state.entities.0.get(&entity)?.health?;
            if health.1 == 0 || health.0 == 0 {
                return None;
            }
            let fraction = health.0 as f32 / health.1 as f32;
            (fraction <= emergency_threshold).then_some((entity, fraction))
        })
        .min_by(|(left_id, left), (right_id, right)| {
            left.total_cmp(right).then_with(|| left_id.cmp(right_id))
        })
        .map(|(entity, _)| entity)?;

    let families = HEAL_FAMILIES
        .iter()
        .find(|(candidate, _)| *candidate == class_id)?
        .1;
    let now_ms = Millis::wall_clock_now().0;
    families.iter().find_map(|family| {
        crate::combat::spells::family_spells(*family)?
            .iter()
            .rev()
            .find(|spell| {
                snapshot.state.capabilities.spells.contains(spell)
                    && crate::combat::readiness::check_spell_readiness(
                        snapshot,
                        **spell,
                        Some(target),
                        now_ms,
                    )
                    .is_ok()
            })
            .copied()
            .map(|spell| (spell, target))
    })
}

pub fn supported_class_trees() -> impl Iterator<Item = (u8, u8, Vec<(u8, u32)>)> {
    combat_catalog()
        .classes
        .iter()
        .flat_map(|(&class, policy)| {
            policy.trees.iter().filter_map(move |(&tree, tree_policy)| {
                let power_profiles: Vec<_> = tree_policy
                    .power_policies
                    .iter()
                    .filter_map(|(&power_type, priorities)| {
                        priorities
                            .first()
                            .and_then(|family_id| crate::combat::spells::family_spells(*family_id))
                            .and_then(|spells| spells.first())
                            .map(|spell| (power_type, *spell))
                    })
                    .collect();
                (!power_profiles.is_empty()).then_some((class, tree, power_profiles))
            })
        })
}

fn player_power(snapshot: &Snapshot) -> Option<(u8, u32)> {
    let player = snapshot.state.session.character_guid.map(EntityId)?;
    let entity = snapshot.state.entities.0.get(&player)?;
    Some((entity.power_type?, entity.power?.0))
}

fn is_wand_caster(class_id: u8) -> bool {
    matches!(class_id, 5 | 8 | 9)
}

fn deferred(reason: &'static str) -> CombatDecision {
    CombatDecision::Deferred { reason }
}

pub fn is_oom(snapshot: &Snapshot) -> bool {
    player_power(snapshot).is_some_and(|(power_type, current)| power_type == 0 && current == 0)
}

pub fn has_equipped_wand(snapshot: &Snapshot) -> bool {
    snapshot
        .state
        .inventory
        .equipped_ranged_item
        .is_some_and(|item| wand_catalog().wand_items.binary_search(&item).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{
        ActiveCastState, AuthoritativeState,
        entities::{EntityKind, EntityState},
        group::GroupMember,
    };

    fn state(
        class_id: u8,
        tree: u8,
        power_type: u8,
        power: u32,
        target: EntityId,
    ) -> AuthoritativeState {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.capabilities.class_id = Some(class_id);
        state.capabilities.specialization_tree = Some(tree);
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
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
            target,
            EntityState {
                id: target,
                ..Default::default()
            },
        );
        state
    }

    #[test]
    fn every_wotlk_class_and_tree_has_a_grounded_priority() {
        let entries: Vec<_> = supported_class_trees().collect();
        assert_eq!(entries.len(), 30);
        for (class_id, tree, profiles) in entries {
            for (power_type, spell) in profiles {
                let target = EntityId(9);
                let mut state = state(class_id, tree, power_type, 1000, target);
                state.capabilities.spells.insert(spell);
                if let Some(requirement) = crate::combat::spells::metadata(spell)
                    .map(|metadata| metadata.equipment)
                    .filter(|requirement| requirement.class_id >= 0)
                {
                    state.inventory.equipment_slots_authoritative = true;
                    let equipped = crate::combat::spells::item_ids().find(|item_id| {
                        crate::combat::spells::item_metadata(*item_id).is_some_and(|item| {
                            item.class_id == requirement.class_id as u32
                                && (requirement.subclass_mask <= 0
                                    || requirement.subclass_mask as u32
                                        & (1u32.checked_shl(item.subclass).unwrap_or(0))
                                        != 0)
                                && (requirement.inventory_mask <= 0
                                    || requirement.inventory_mask as u32
                                        & (1u32.checked_shl(item.inventory_type).unwrap_or(0))
                                        != 0)
                        })
                    });
                    if let Some(item) = equipped {
                        state.inventory.equipped_items.insert(15, item);
                    }
                }
                let decision = select(&Snapshot::from_state(&state), target);
                let readiness = crate::combat::readiness::check_spell_readiness(
                    &Snapshot::from_state(&state),
                    spell,
                    Some(target),
                    Millis::wall_clock_now().0,
                );
                if readiness.is_ok() {
                    assert_eq!(
                        decision,
                        CombatDecision::Cast { spell, target },
                        "class={class_id}, tree={tree}, power={power_type}"
                    );
                } else if let Some(metadata) = crate::combat::spells::metadata(spell) {
                    if metadata.requirements.target_creature_type != 0 {
                        assert_ne!(
                            decision,
                            CombatDecision::Cast { spell, target },
                            "class={class_id}, tree={tree}, power={power_type} must not guess target creature type"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn specialization_resource_and_spell_readiness_fail_closed() {
        let target = EntityId(9);
        let mut no_tree = state(8, 0, 0, 100, target);
        no_tree.capabilities.specialization_tree = None;
        assert_eq!(
            select(&Snapshot::from_state(&no_tree), target),
            deferred("specialization_not_authoritative")
        );

        let mut unknown_power = state(8, 0, 0, 100, target);
        unknown_power
            .entities
            .0
            .get_mut(&EntityId(1))
            .unwrap()
            .power = None;
        assert_eq!(
            select(&Snapshot::from_state(&unknown_power), target),
            deferred("combat_resource_state_unknown")
        );

        let mut no_spell = state(8, 0, 0, 100, target);
        assert_eq!(
            select(&Snapshot::from_state(&no_spell), target),
            deferred("no_safe_offensive_fallback")
        );

        let spell = supported_class_trees()
            .find(|(class, tree, _)| (*class, *tree) == (8, 0))
            .unwrap()
            .2[0]
            .1;
        no_spell.capabilities.spells.insert(spell);
        no_spell
            .capabilities
            .spell_cooldowns
            .insert(spell, u64::MAX);
        assert_eq!(
            select(&Snapshot::from_state(&no_spell), target),
            deferred("no_safe_offensive_fallback")
        );

        no_spell.entities.0.get_mut(&EntityId(1)).unwrap().power = Some((0, 100));
        no_spell.inventory.equipment_authoritative = true;
        assert_eq!(
            select(&Snapshot::from_state(&no_spell), target),
            deferred("no_safe_offensive_fallback")
        );

        no_spell.capabilities.spells.insert(5019);
        assert_eq!(
            select(&Snapshot::from_state(&no_spell), target),
            deferred("no_safe_offensive_fallback")
        );
        no_spell.inventory.equipped_ranged_item = wand_catalog().wand_items.first().copied();
        assert_eq!(
            select(&Snapshot::from_state(&no_spell), target),
            CombatDecision::Wand { target }
        );
        assert_eq!(
            select_action(&Snapshot::from_state(&no_spell), target)
                .unwrap()
                .command,
            GameplayCommand::Cast {
                spell: 5019,
                target: Some(target),
            }
        );

        no_spell.entities.0.get_mut(&EntityId(1)).unwrap().power = Some((100, 100));
        assert_eq!(
            select(&Snapshot::from_state(&no_spell), target),
            CombatDecision::Wand { target }
        );
    }

    #[test]
    fn caster_uses_known_ready_ranged_attack_when_priority_spell_is_unknown() {
        let target = EntityId(9);
        let mut state = state(8, 0, 0, 100, target);
        state.capabilities.spells.insert(116); // Mage Frostbolt, outside Arcane priorities.

        assert_eq!(
            select(&Snapshot::from_state(&state), target),
            CombatDecision::Cast { spell: 116, target }
        );
        assert_eq!(
            select_action(&Snapshot::from_state(&state), target)
                .unwrap()
                .command,
            GameplayCommand::Cast {
                spell: 116,
                target: Some(target),
            }
        );
    }

    #[test]
    fn interrupt_precedes_ranged_attack_fallback() {
        let target = EntityId(9);
        let mut state = state(8, 0, 0, 100, target);
        state.capabilities.spells.extend([116, 2139]);
        state.entities.0.get_mut(&target).unwrap().kind = EntityKind::Unit;
        state.entities.0.get_mut(&target).unwrap().hostile = true;
        state.active_casts.insert(
            target,
            ActiveCastState {
                spell: 133,
                started_at_ms: 1,
                ends_at_ms: u64::MAX,
            },
        );

        assert_eq!(
            select_action(&Snapshot::from_state(&state), target)
                .unwrap()
                .command,
            GameplayCommand::Cast {
                spell: 2139,
                target: Some(target),
            }
        );
    }

    #[test]
    fn emergency_heal_precedes_ranged_attack_fallback() {
        let target = EntityId(9);
        let mut state = state(5, 0, 0, 100, target);
        state.capabilities.spells.extend([585, 2061]);
        state.entities.0.get_mut(&EntityId(1)).unwrap().health = Some((20, 100));

        assert_eq!(
            select_action(&Snapshot::from_state(&state), target)
                .unwrap()
                .command,
            GameplayCommand::Cast {
                spell: 2061,
                target: Some(EntityId(1)),
            }
        );
    }

    #[test]
    fn ranged_caster_does_not_use_melee_without_melee_profile() {
        let target = EntityId(9);
        let mut state = state(8, 0, 0, 0, target);
        state.inventory.equipment_authoritative = true;

        assert_eq!(
            select(&Snapshot::from_state(&state), target),
            deferred("no_safe_offensive_fallback")
        );
    }

    #[test]
    fn assigned_tank_can_use_basic_melee_fallback() {
        let target = EntityId(9);
        let mut state = state(8, 0, 0, 0, target);
        state.group.members.push(GroupMember {
            entity: EntityId(1),
            role: Some("main tank".into()),
            ..Default::default()
        });

        assert_eq!(
            select(&Snapshot::from_state(&state), target),
            CombatDecision::Melee { target }
        );
    }

    #[test]
    fn melee_class_uses_basic_attack_when_no_priority_or_ranged_attack_is_ready() {
        let target = EntityId(9);
        let (power_type, _) = supported_class_trees()
            .find(|(class, tree, _)| (*class, *tree) == (1, 0))
            .unwrap()
            .2[0];
        let state = state(1, 0, power_type, 100, target);

        assert_eq!(
            select(&Snapshot::from_state(&state), target),
            CombatDecision::Melee { target }
        );
    }

    #[test]
    fn spell_catalog_marks_damage_and_dot_spells_but_not_buffs_or_pet_summons() {
        assert!(crate::combat::spells::metadata(116).unwrap().attack_spell);
        assert!(
            crate::combat::spells::metadata(172)
                .unwrap()
                .damage_over_time
        );
        assert!(!crate::combat::spells::metadata(17).unwrap().attack_spell);
        assert!(!crate::combat::spells::metadata(688).unwrap().attack_spell);
    }
}
