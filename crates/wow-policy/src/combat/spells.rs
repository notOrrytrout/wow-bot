use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};
use wow_state::Snapshot;

#[derive(Clone, Copy, Debug, Deserialize)]
struct SpellRange {
    hostile_min: f32,
    friendly_min: f32,
    hostile_max: f32,
    friendly_max: f32,
}

#[derive(Deserialize)]
struct SpellRangeCatalog {
    spell_range_indices: Vec<(u32, u32)>,
    ranges: BTreeMap<u32, SpellRange>,
    stealth_required_spells: Vec<u32>,
    stealth_aura_spells: Vec<u32>,
}

static SPELL_RANGES: OnceLock<SpellRangeCatalog> = OnceLock::new();

fn spell_range_catalog() -> &'static SpellRangeCatalog {
    SPELL_RANGES.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/spell-ranges.json"))
            .expect("generated spell range catalog")
    })
}

/// Return hostile or friendly min/max range data for a spell from the 3.3.5a DBC catalog.
pub fn range_for(spell: u32, hostile_target: bool) -> Option<(f32, f32)> {
    let catalog = spell_range_catalog();
    let index = catalog
        .spell_range_indices
        .binary_search_by_key(&spell, |(spell_id, _)| *spell_id)
        .ok()
        .map(|position| catalog.spell_range_indices[position].1)?;
    let range = catalog.ranges.get(&index)?;
    let (minimum, maximum) = if hostile_target {
        (range.hostile_min, range.hostile_max)
    } else {
        (range.friendly_min, range.friendly_max)
    };
    (maximum > 0.0).then_some((minimum, maximum))
}

pub fn requires_stealth(spell: u32) -> bool {
    spell_range_catalog()
        .stealth_required_spells
        .binary_search(&spell)
        .is_ok()
}

pub fn player_has_stealth(snapshot: &Snapshot) -> bool {
    let Some(player) = snapshot
        .state
        .session
        .character_guid
        .map(wow_domain::EntityId)
    else {
        return false;
    };
    snapshot.state.auras.spells(player).iter().any(|spell| {
        spell_range_catalog()
            .stealth_aura_spells
            .binary_search(spell)
            .is_ok()
    })
}

#[derive(Clone, Copy, Debug)]
pub struct SpellSemantics {
    pub spell: u32,
    pub max_range: f32,
    pub hostile: bool,
    pub breaks_cc: bool,
    pub interrupt: bool,
}
pub fn knows_spell(state: &Snapshot, spell: u32) -> bool {
    state.state.capabilities.spells.contains(&spell)
}
pub fn mechanically_legal(state: &Snapshot, sem: SpellSemantics, distance: f32) -> bool {
    knows_spell(state, sem.spell) && distance.is_finite() && distance <= sem.max_range
}
