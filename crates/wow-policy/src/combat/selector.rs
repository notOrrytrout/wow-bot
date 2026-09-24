use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};
use wow_domain::EntityId;
use wow_state::Snapshot;

#[derive(Clone, Debug, Deserialize)]
struct CombatCatalog {
    classes: BTreeMap<u8, Vec<SpellFamily>>,
}
#[derive(Clone, Debug, Deserialize)]
struct SpellFamily {
    #[serde(rename = "root")]
    _root: u32,
    spells: Vec<RankedSpell>,
}
#[derive(Clone, Copy, Debug, Deserialize)]
struct RankedSpell {
    rank: u32,
    spell: u32,
}
#[derive(Clone, Debug, Deserialize)]
struct WandCatalog {
    wand_items: Vec<u32>,
}

static COMBAT: OnceLock<CombatCatalog> = OnceLock::new();
static WANDS: OnceLock<WandCatalog> = OnceLock::new();
fn combat_catalog() -> &'static CombatCatalog {
    COMBAT.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/combat-spells.json"))
            .expect("combat spell catalog")
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

pub fn select(snapshot: &Snapshot, target: EntityId) -> CombatDecision {
    let Some(class_id) = snapshot.state.capabilities.class_id else {
        return CombatDecision::Deferred {
            reason: "class_not_authoritative",
        };
    };
    let caster = matches!(class_id, 5 | 8 | 9);
    if !caster {
        return CombatDecision::Melee { target };
    }

    if let Some(spell) = highest_known_offensive_spell(snapshot, class_id) {
        if !is_oom(snapshot) {
            if crate::combat::readiness::check_spell_readiness(
                snapshot,
                spell,
                Some(target),
                wall_clock_ms(),
            )
            .is_ok()
            {
                return CombatDecision::Cast { spell, target };
            }
        }
    }

    if has_equipped_wand(snapshot) {
        // Shoot (Wand) in 3.3.5a. The server validates the equipped ranged weapon.
        return CombatDecision::Wand { target };
    }

    if is_oom(snapshot) && !snapshot.state.inventory.equipment_authoritative {
        return CombatDecision::Deferred {
            reason: "oom_but_ranged_slot_not_authoritative",
        };
    }
    if is_oom(snapshot) {
        CombatDecision::Melee { target }
    } else {
        CombatDecision::Deferred {
            reason: "caster_has_mana_but_no_known_offensive_spell",
        }
    }
}

fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn is_oom(snapshot: &Snapshot) -> bool {
    let Some(player) = snapshot.state.session.character_guid.map(EntityId) else {
        return false;
    };
    snapshot
        .state
        .entities
        .0
        .get(&player)
        .filter(|entity| entity.power_type == Some(0))
        .and_then(|entity| entity.power)
        .is_some_and(|(current, _)| current == 0)
}

pub fn has_equipped_wand(snapshot: &Snapshot) -> bool {
    snapshot
        .state
        .inventory
        .equipped_ranged_item
        .is_some_and(|item| wand_catalog().wand_items.binary_search(&item).is_ok())
}

fn highest_known_offensive_spell(snapshot: &Snapshot, class_id: u8) -> Option<u32> {
    combat_catalog()
        .classes
        .get(&class_id)?
        .iter()
        .flat_map(|family| family.spells.iter())
        .filter(|ranked| snapshot.state.capabilities.spells.contains(&ranked.spell))
        .max_by_key(|ranked| (ranked.rank, ranked.spell))
        .map(|ranked| ranked.spell)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::{
        AuthoritativeState,
        entities::{EntityKind, EntityState},
    };

    fn caster_state(class_id: u8, mana: u32) -> AuthoritativeState {
        let mut state = AuthoritativeState::default();
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.capabilities.class_id = Some(class_id);
        state.entities.0.insert(
            EntityId(1),
            EntityState {
                id: EntityId(1),
                kind: EntityKind::Player,
                power: Some((mana, 100)),
                power_type: Some(0),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                kind: EntityKind::Unit,
                ..Default::default()
            },
        );
        state
    }

    #[test]
    fn warlock_prefers_shadow_bolt_while_mana_available() {
        let mut state = caster_state(9, 100);
        state.capabilities.spells.extend([686, 695]);
        assert_eq!(
            select(&Snapshot::from_state(&state), EntityId(9)),
            CombatDecision::Cast {
                spell: 695,
                target: EntityId(9)
            }
        );
    }
    #[test]
    fn oom_caster_prefers_wand_to_melee() {
        let mut state = caster_state(9, 0);
        state.inventory.equipment_authoritative = true;
        state.inventory.equipped_ranged_item = Some(5207);
        assert_eq!(
            select(&Snapshot::from_state(&state), EntityId(9)),
            CombatDecision::Wand {
                target: EntityId(9)
            }
        );
    }
    #[test]
    fn oom_caster_without_wand_may_melee() {
        let mut state = caster_state(8, 0);
        state.inventory.equipment_authoritative = true;
        assert_eq!(
            select(&Snapshot::from_state(&state), EntityId(9)),
            CombatDecision::Melee {
                target: EntityId(9)
            }
        );
    }
    #[test]
    fn oom_caster_waits_until_ranged_slot_is_authoritative() {
        let state = caster_state(9, 0);
        assert_eq!(
            select(&Snapshot::from_state(&state), EntityId(9)),
            CombatDecision::Deferred {
                reason: "oom_but_ranged_slot_not_authoritative"
            }
        );
    }
}
