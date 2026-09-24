use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::OnceLock,
    time::{Duration, Instant},
};
use wow_domain::{EntityId, Millis};
use wow_state::Snapshot;

#[derive(Clone, Debug, Deserialize)]
struct Catalog {
    policies: Vec<BuffFamilyPolicy>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct BuffFamilyPolicy {
    pub class_id: u8,
    pub family: String,
    pub party: bool,
    pub spells: Vec<RankedSpell>,
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct RankedSpell {
    pub spell: u32,
    pub rank: u32,
    pub level: u32,
    pub strength: u32,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();
fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/buff-families.json"))
            .expect("embedded buff catalog must parse")
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaintenanceDecision {
    Satisfied,
    Cast {
        family: String,
        spell: u32,
        target: EntityId,
    },
    Deferred {
        family: String,
        reason: &'static str,
    },
}

pub fn decide_next(
    snapshot: &Snapshot,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
    include_party: bool,
) -> MaintenanceDecision {
    let Some(class_id) = snapshot.state.capabilities.class_id else {
        return MaintenanceDecision::Deferred {
            family: "player_class".into(),
            reason: "class_not_authoritative",
        };
    };
    let Some(player_raw) = snapshot.state.session.character_guid else {
        return MaintenanceDecision::Deferred {
            family: "player".into(),
            reason: "player_guid_not_authoritative",
        };
    };
    let player = EntityId(player_raw);
    if !snapshot.state.session.in_world {
        return MaintenanceDecision::Deferred {
            family: "session".into(),
            reason: "not_in_world",
        };
    }
    if !snapshot.state.auras.by_entity.contains_key(&player) {
        return MaintenanceDecision::Deferred {
            family: "auras".into(),
            reason: "player_auras_not_authoritative",
        };
    }
    if snapshot
        .state
        .entities
        .0
        .get(&player)
        .and_then(|entity| entity.health)
        .is_some_and(|(health, _)| health == 0)
    {
        return MaintenanceDecision::Deferred {
            family: "life".into(),
            reason: "player_dead",
        };
    }
    if snapshot.state.control.mover.is_some() {
        return MaintenanceDecision::Deferred {
            family: "control".into(),
            reason: "controlled_mover_active",
        };
    }
    if snapshot.state.position.moving {
        return MaintenanceDecision::Deferred {
            family: "movement".into(),
            reason: "player_moving",
        };
    }

    for family in catalog()
        .policies
        .iter()
        .filter(|policy| policy.class_id == class_id)
    {
        let Some(desired) = highest_known(snapshot, family) else {
            continue;
        };
        let spell = desired.spell;
        if !has_same_or_better(snapshot, player, family, desired.strength) {
            let Some(decision) = cast_decision(snapshot, family, spell, player, retry_after, now)
            else {
                continue;
            };
            return decision;
        }
        if include_party && family.party {
            for member in snapshot
                .state
                .group
                .members
                .iter()
                .filter(|member| member.online && member.entity != player)
            {
                if !snapshot.state.auras.by_entity.contains_key(&member.entity) {
                    continue;
                }
                if !party_member_nearby(snapshot, player, member.entity) {
                    continue;
                }
                if has_same_or_better(snapshot, member.entity, family, desired.strength) {
                    continue;
                }
                if let Some(decision) =
                    cast_decision(snapshot, family, spell, member.entity, retry_after, now)
                {
                    return decision;
                }
            }
        }
    }
    MaintenanceDecision::Satisfied
}

fn cast_decision(
    snapshot: &Snapshot,
    family: &BuffFamilyPolicy,
    spell: u32,
    target: EntityId,
    retry_after: &BTreeMap<(u32, EntityId), Instant>,
    now: Instant,
) -> Option<MaintenanceDecision> {
    if retry_after
        .get(&(spell, target))
        .is_some_and(|deadline| *deadline > now)
    {
        return None;
    }
    if let Err(reason) = crate::combat::readiness::check_spell_readiness(
        snapshot,
        spell,
        Some(target),
        Millis::wall_clock_now().0,
    ) {
        return Some(MaintenanceDecision::Deferred {
            family: family.family.clone(),
            reason: readiness_reason(reason),
        });
    }
    Some(MaintenanceDecision::Cast {
        family: family.family.clone(),
        spell,
        target,
    })
}

fn readiness_reason(reason: crate::combat::readiness::SpellUnavailableReason) -> &'static str {
    use crate::combat::readiness::SpellUnavailableReason::*;
    match reason {
        Unknown => "spell_unknown",
        Cooldown => "spell_cooldown",
        InsufficientPower => "insufficient_power",
        TargetNotAuthoritative => "target_not_authoritative",
        NotInWorld => "not_in_world",
    }
}

pub fn retry_deadline(now: Instant) -> Instant {
    now + Duration::from_secs(5)
}

fn highest_known<'a>(snapshot: &Snapshot, family: &'a BuffFamilyPolicy) -> Option<&'a RankedSpell> {
    family
        .spells
        .iter()
        .filter(|ranked| snapshot.state.capabilities.spells.contains(&ranked.spell))
        .max_by_key(|ranked| (ranked.strength, ranked.level, ranked.rank, ranked.spell))
}

fn has_same_or_better(
    snapshot: &Snapshot,
    target: EntityId,
    family: &BuffFamilyPolicy,
    desired_strength: u32,
) -> bool {
    let active = snapshot.state.auras.spells(target);
    family
        .spells
        .iter()
        .any(|ranked| active.contains(&ranked.spell) && ranked.strength >= desired_strength)
}

fn party_member_nearby(snapshot: &Snapshot, player: EntityId, member: EntityId) -> bool {
    let player_pos = snapshot
        .state
        .entities
        .0
        .get(&player)
        .and_then(|entity| entity.position)
        .or(snapshot.state.position.player);
    let member_pos = snapshot
        .state
        .entities
        .0
        .get(&member)
        .and_then(|entity| entity.position);
    player_pos
        .zip(member_pos)
        .is_some_and(|(player_pos, member_pos)| {
            player_pos.map == member_pos.map && player_pos.point.distance(member_pos.point) <= 20.0
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{AuthoritativeState, Snapshot, auras::AuraInstance};
    #[test]
    fn mage_chooses_highest_known_intellect_and_stops_when_family_present() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.extend([1459, 1460, 42995]);
        state.auras.by_entity.entry(EntityId(7)).or_default();
        let snap = Snapshot::from_state(&state);
        let now = Instant::now();
        let retry = BTreeMap::new();
        assert_eq!(
            decide_next(&snap, &retry, now, false),
            MaintenanceDecision::Cast {
                family: "arcane_intellect".into(),
                spell: 42995,
                target: EntityId(7)
            }
        );
        state
            .auras
            .by_entity
            .entry(EntityId(7))
            .or_default()
            .insert(
                1,
                AuraInstance {
                    slot: 1,
                    spell: 42995,
                    positive: Some(true),
                    caster: Some(EntityId(7)),
                    max_duration_ms: None,
                    remaining_ms: None,
                },
            );
        assert_eq!(
            decide_next(&Snapshot::from_state(&state), &retry, now, false),
            MaintenanceDecision::Satisfied
        );
    }
    #[test]
    fn retry_backoff_defers_duplicate_cast_attempt() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.insert(1459);
        state.auras.by_entity.entry(EntityId(7)).or_default();
        let snap = Snapshot::from_state(&state);
        let now = Instant::now();
        let mut retry = BTreeMap::new();
        retry.insert((1459, EntityId(7)), now + Duration::from_secs(10));
        assert_eq!(
            decide_next(&snap, &retry, now, false),
            MaintenanceDecision::Satisfied
        );
    }

    #[test]
    fn controlled_mover_suspends_normal_player_maintenance() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.insert(1459);
        state.auras.by_entity.entry(EntityId(7)).or_default();
        state.control.mover = Some(EntityId(99));
        assert!(matches!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::Deferred {
                reason: "controlled_mover_active",
                ..
            }
        ));
    }

    #[test]
    fn low_level_warlock_maintains_demon_skin_before_demon_armor_is_known() {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(9);
        state.capabilities.spells.insert(687);
        state.auras.by_entity.entry(EntityId(7)).or_default();
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                false
            ),
            MaintenanceDecision::Cast {
                family: "warlock_armor".into(),
                spell: 687,
                target: EntityId(7)
            }
        );
    }

    #[test]
    fn party_member_out_of_range_or_already_satisfied_is_skipped() {
        use wow_domain::{Vec3, WorldPosition};
        use wow_state::{
            entities::{EntityKind, EntityState},
            group::{GroupMember, GroupState},
        };
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(7);
        state.capabilities.class_id = Some(8);
        state.capabilities.spells.insert(1459);
        state
            .auras
            .by_entity
            .entry(EntityId(7))
            .or_default()
            .insert(
                1,
                AuraInstance {
                    slot: 1,
                    spell: 1459,
                    positive: Some(true),
                    caster: Some(EntityId(7)),
                    max_duration_ms: None,
                    remaining_ms: None,
                },
            );
        state.entities.0.insert(
            EntityId(7),
            EntityState {
                id: EntityId(7),
                kind: EntityKind::Player,
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(0.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(8),
            EntityState {
                id: EntityId(8),
                kind: EntityKind::Player,
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(100.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        state.auras.by_entity.entry(EntityId(8)).or_default();
        state.group = GroupState {
            members: vec![GroupMember {
                entity: EntityId(8),
                online: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            decide_next(
                &Snapshot::from_state(&state),
                &BTreeMap::new(),
                Instant::now(),
                true
            ),
            MaintenanceDecision::Satisfied
        );
    }
}
