use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};
use wow_domain::Vec3;
use wow_state::quests::QuestTargetKind;

#[derive(Clone, Debug, Deserialize)]
struct StaticQuest { title: String, targets: Vec<StaticTarget>, items: Vec<StaticItem> }
#[derive(Clone, Debug, Deserialize)]
struct StaticTarget { kind: String, entry: u32, required: u32, text: String }
#[derive(Clone, Debug, Deserialize)]
struct StaticItem { item: u32, required: u32 }
#[derive(Clone, Debug, Deserialize)]
struct StaticQuestTool { entry: u32 }
#[derive(Clone, Debug, Deserialize)]
struct StaticHints {
    format_version: u32,
    quests: BTreeMap<u32, StaticQuest>,
    quest_ends: BTreeMap<u32, Vec<(String, u32)>>,
    creature_spawns: BTreeMap<u32, Vec<[f32; 5]>>,
    gameobject_spawns: BTreeMap<u32, Vec<[f32; 5]>>,
    item_sources: BTreeMap<u32, Vec<(String, u32)>>,
    #[serde(default)]
    spell_targets: BTreeMap<u32, Vec<u32>>,
    #[serde(default)]
    quest_tools: BTreeMap<u32, Vec<StaticQuestTool>>,
}

static HINTS: OnceLock<StaticHints> = OnceLock::new();
fn hints() -> &'static StaticHints {
    HINTS.get_or_init(|| serde_json::from_str(include_str!("../../data/azerothcore-quest-hints.json"))
        .expect("embedded AzerothCore quest hints must be valid JSON"))
}

#[derive(Clone, Debug)]
pub struct StaticQuestDefinition {
    pub title: String,
    pub targets: Vec<(QuestTargetKind, u32, u32, String)>,
    pub items: Vec<(u32, u32)>,
}

pub fn quest_definition(quest: u32) -> Option<StaticQuestDefinition> {
    let q = hints().quests.get(&quest)?;
    Some(StaticQuestDefinition {
        title: q.title.clone(),
        targets: q.targets.iter().filter_map(|target| {
            let kind = match target.kind.as_str() {
                "creature" => QuestTargetKind::Creature,
                "gameobject" => QuestTargetKind::GameObject,
                _ => return None,
            };
            Some((kind, target.entry, target.required, target.text.clone()))
        }).collect(),
        items: q.items.iter().map(|item| (item.item, item.required)).collect(),
    })
}

pub fn nearest_target_spawn(kind: QuestTargetKind, entry: u32, map: u32, from: Vec3) -> Option<Vec3> {
    let spawns = match kind {
        QuestTargetKind::Creature => hints().creature_spawns.get(&entry),
        QuestTargetKind::GameObject => hints().gameobject_spawns.get(&entry),
    }?;
    nearest(spawns, map, from)
}

pub fn item_source_entries(item: u32) -> Vec<(QuestTargetKind, u32)> {
    hints().item_sources.get(&item).into_iter().flat_map(|sources| sources.iter()).filter_map(|(kind, entry)| {
        let kind = match kind.as_str() {
            "creature" => QuestTargetKind::Creature,
            "gameobject" => QuestTargetKind::GameObject,
            _ => return None,
        };
        Some((kind, *entry))
    }).collect()
}

pub fn quest_tool_entries(quest: u32) -> Vec<u32> {
    hints().quest_tools.get(&quest).into_iter().flat_map(|tools| tools.iter()).map(|tool| tool.entry).collect()
}

pub fn nearest_quest_tool(quest: u32, map: u32, from: Vec3) -> Option<Vec3> {
    let mut best: Option<(f32, Vec3)> = None;
    for tool in hints().quest_tools.get(&quest)? {
        if let Some(point) = nearest_target_spawn(QuestTargetKind::GameObject, tool.entry, map, from) {
            let distance = from.distance(point);
            if best.as_ref().is_none_or(|(known, _)| distance < *known) { best = Some((distance, point)); }
        }
    }
    best.map(|(_, point)| point)
}

pub fn observed_control_spells_for_target(entry: u32) -> &'static [u32] {
    hints().spell_targets.get(&entry).map(Vec::as_slice).unwrap_or(&[])
}

pub fn nearest_item_source(item: u32, map: u32, from: Vec3) -> Option<Vec3> {
    let mut best: Option<(f32, Vec3)> = None;
    for (kind, entry) in hints().item_sources.get(&item)? {
        let kind = match kind.as_str() {
            "creature" => QuestTargetKind::Creature,
            "gameobject" => QuestTargetKind::GameObject,
            _ => continue,
        };
        if let Some(point) = nearest_target_spawn(kind, *entry, map, from) {
            let distance = from.distance(point);
            if best.as_ref().is_none_or(|(known, _)| distance < *known) { best = Some((distance, point)); }
        }
    }
    best.map(|(_, point)| point)
}

pub fn nearest_turn_in(quest: u32, map: u32, from: Vec3) -> Option<Vec3> {
    let mut best: Option<(f32, Vec3)> = None;
    for (kind, entry) in hints().quest_ends.get(&quest)? {
        let kind = match kind.as_str() {
            "creature" => QuestTargetKind::Creature,
            "gameobject" => QuestTargetKind::GameObject,
            _ => continue,
        };
        if let Some(point) = nearest_target_spawn(kind, *entry, map, from) {
            let distance = from.distance(point);
            if best.as_ref().is_none_or(|(known, _)| distance < *known) { best = Some((distance, point)); }
        }
    }
    best.map(|(_, point)| point)
}

fn nearest(spawns: &[[f32; 5]], map: u32, from: Vec3) -> Option<Vec3> {
    spawns.iter().filter_map(|spawn| {
        if spawn[0] as u32 != map { return None; }
        let point = Vec3::new(spawn[1], spawn[2], spawn[3]);
        point.is_finite().then_some((from.distance(point), point))
    }).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, point)| point)
}

pub fn format_version() -> u32 { hints().format_version }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn embedded_hints_load_and_have_classic_quest_data() {
        assert_eq!(format_version(), 3);
        assert!(quest_definition(33).is_some());
        assert!(observed_control_spells_for_target(28525).contains(&51858));
        assert!(quest_tool_entries(12641).contains(&191609));
        assert_eq!(quest_tool_activation_spell(12641), Some(6247));
        assert_eq!(scripted_item_use(5441), Some((16114, 19938, 10556, 1)));
    }
}

/// Scripted quest item interactions that AzerothCore reports through ordinary
/// quest credit fields even though the player must use an item on a living target.
/// These facts come from the supplied AzerothCore scripts and reviewed stock-client behavior.

pub const fn quest_tool_activation_spell(quest: u32) -> Option<u32> {
    match quest {
        // Death Comes From On High: stock-client flow casts the Eye-control spell
        // on gameobject 191609 before the controlled Eye appears.
        12641 => Some(6247),
        _ => None,
    }
}

pub const fn scripted_item_use(quest: u32) -> Option<(u32, u32, u32, u8)> {
    match quest {
        // Lazy Peons: Foreman's Blackjack (16114), spell Awaken Peon (19938),
        // living Lazy Peon entry 10556. Captured stock client uses cast_count=1.
        5441 => Some((16114, 19938, 10556, 1)),
        _ => None,
    }
}
