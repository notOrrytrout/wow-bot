use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};
use wow_state::Snapshot;

#[derive(Clone, Debug, Deserialize)]
pub struct SpellMetadata {
    pub id: u32,
    pub name: String,
    pub rank: String,
    pub family_id: u32,
    pub classes: Vec<u8>,
    pub power_type: i32,
    pub cost: SpellCost,
    pub cooldown_ms: u32,
    pub category_cooldown_ms: u32,
    pub global_cooldown: GlobalCooldown,
    pub cast_time_ms: Option<u32>,
    pub duration_ms: Option<i32>,
    pub range: Option<SpellRange>,
    pub runes: Option<RuneCost>,
    pub reagents: Vec<ReagentCost>,
    pub equipment: EquipmentRequirement,
    pub requirements: SpellRequirements,
    pub spell_family: u32,
    pub family_flags: [u32; 3],
    pub requires_combo_points: bool,
    pub cost_spell_modifiers: Vec<CostSpellModifier>,
    pub school_mask: u32,
    pub attack_spell: bool,
    pub damage_over_time: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CostSpellModifier {
    pub aura_type: u32,
    pub base_points: i32,
    pub family_flags: [u32; 3],
    pub percent: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ItemMetadata {
    #[serde(rename = "class")]
    pub class_id: u32,
    pub subclass: u32,
    pub inventory_type: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ShapeshiftFormMetadata {
    pub flags: u32,
    pub attack_speed_ms: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SpellCost {
    pub base: u32,
    pub per_level: u32,
    pub percent: u32,
    pub per_second: u32,
    pub per_second_per_level: u32,
    pub use_all_power: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct GlobalCooldown {
    pub category: u32,
    pub duration_ms: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct SpellRange {
    pub hostile_min: f32,
    pub friendly_min: f32,
    pub hostile_max: f32,
    pub friendly_max: f32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct RuneCost {
    pub blood: u32,
    pub frost: u32,
    pub unholy: u32,
    pub runic_power_gain: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ReagentCost {
    pub item: i32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct EquipmentRequirement {
    #[serde(rename = "class")]
    pub class_id: i32,
    pub subclass_mask: i32,
    pub inventory_mask: i32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SpellRequirements {
    pub stance_mask: u32,
    pub stance_exclude: u32,
    pub targets: u32,
    pub target_creature_type: u32,
    pub spell_focus: u32,
    pub facing_flags: u32,
    pub caster_aura_state: u32,
    pub target_aura_state: u32,
    pub caster_aura_state_not: u32,
    pub target_aura_state_not: u32,
    pub caster_aura_spell: u32,
    pub target_aura_spell: u32,
    pub exclude_caster_aura_spell: u32,
    pub exclude_target_aura_spell: u32,
    pub attributes: u32,
    pub attributes_ex: u32,
    pub attributes_ex2: u32,
    pub attributes_ex3: u32,
    pub attributes_ex4: u32,
    pub attributes_ex5: u32,
    pub attributes_ex6: u32,
    pub attributes_ex7: u32,
}

#[derive(Deserialize)]
struct SpellCatalog {
    format_version: u32,
    spells: Vec<SpellMetadata>,
    spell_families: BTreeMap<u32, Vec<u32>>,
    stealth_required_spells: Vec<u32>,
    stealth_aura_spells: Vec<u32>,
    items: BTreeMap<u32, ItemMetadata>,
    shapeshift_forms: BTreeMap<u8, ShapeshiftFormMetadata>,
}

struct SpellCatalogIndex {
    spells: BTreeMap<u32, SpellMetadata>,
    families: BTreeMap<u32, Vec<u32>>,
    stealth_required_spells: Vec<u32>,
    stealth_aura_spells: Vec<u32>,
    items: BTreeMap<u32, ItemMetadata>,
    shapeshift_forms: BTreeMap<u8, ShapeshiftFormMetadata>,
}

static SPELLS: OnceLock<SpellCatalogIndex> = OnceLock::new();

fn catalog() -> &'static SpellCatalogIndex {
    SPELLS.get_or_init(|| {
        let catalog: SpellCatalog =
            serde_json::from_str(include_str!("../../data/spell-catalog.json"))
                .expect("generated spell catalogue");
        assert_eq!(catalog.format_version, 2);
        SpellCatalogIndex {
            spells: catalog
                .spells
                .into_iter()
                .map(|spell| (spell.id, spell))
                .collect(),
            families: catalog.spell_families,
            stealth_required_spells: catalog.stealth_required_spells,
            stealth_aura_spells: catalog.stealth_aura_spells,
            items: catalog.items,
            shapeshift_forms: catalog.shapeshift_forms,
        }
    })
}

pub fn metadata(spell: u32) -> Option<&'static SpellMetadata> {
    catalog().spells.get(&spell)
}

pub fn family_spells(family_id: u32) -> Option<&'static [u32]> {
    catalog().families.get(&family_id).map(Vec::as_slice)
}

pub fn item_metadata(item: u32) -> Option<&'static ItemMetadata> {
    catalog().items.get(&item)
}

pub fn item_ids() -> impl Iterator<Item = u32> {
    catalog().items.keys().copied()
}

pub fn shapeshift_form_metadata(form: u8) -> Option<&'static ShapeshiftFormMetadata> {
    catalog().shapeshift_forms.get(&form)
}

pub fn range_for(spell: u32, hostile_target: bool) -> Option<(f32, f32)> {
    let range = metadata(spell)?.range?;
    let (minimum, maximum) = if hostile_target {
        (range.hostile_min, range.hostile_max)
    } else {
        (range.friendly_min, range.friendly_max)
    };
    (maximum > 0.0 && minimum.is_finite() && maximum.is_finite()).then_some((minimum, maximum))
}

pub fn requires_stealth(spell: u32) -> bool {
    catalog()
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
    snapshot
        .state
        .auras
        .spells(player)
        .iter()
        .any(|spell| catalog().stealth_aura_spells.binary_search(spell).is_ok())
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
