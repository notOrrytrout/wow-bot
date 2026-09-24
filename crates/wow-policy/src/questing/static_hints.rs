use wow_domain::Vec3;
use wow_infra::world_knowledge::{KnowledgeEntityKind, embedded_azerothcore_catalog};
use wow_state::quests::QuestTargetKind;

pub use wow_infra::world_knowledge::{QuestItemUseRule, QuestSpellRule};

#[derive(Clone, Debug)]
pub struct StaticQuestDefinition {
    pub title: String,
    pub targets: Vec<(QuestTargetKind, u32, u32, String)>,
    pub items: Vec<(u32, u32)>,
}

pub fn quest_definition(quest: u32) -> Option<StaticQuestDefinition> {
    let definition = embedded_azerothcore_catalog().quest(quest)?;
    Some(StaticQuestDefinition {
        title: definition.title.clone(),
        targets: definition
            .targets
            .iter()
            .map(|target| {
                let kind = match target.kind {
                    KnowledgeEntityKind::Creature => QuestTargetKind::Creature,
                    KnowledgeEntityKind::GameObject => QuestTargetKind::GameObject,
                };
                (kind, target.entry, target.required, target.text.clone())
            })
            .collect(),
        items: definition
            .items
            .iter()
            .map(|item| (item.item, item.required))
            .collect(),
    })
}

pub fn nearest_target_spawn(
    kind: QuestTargetKind,
    entry: u32,
    map: u32,
    from: Vec3,
) -> Option<Vec3> {
    embedded_azerothcore_catalog().nearest_spawn(kind, entry, map, from)
}

pub fn target_spawns(kind: QuestTargetKind, entry: u32, map: u32, from: Vec3) -> Vec<Vec3> {
    embedded_azerothcore_catalog().target_spawns(kind, entry, map, from)
}

pub fn item_source_entries(item: u32) -> Vec<(QuestTargetKind, u32)> {
    embedded_azerothcore_catalog().item_source_entries(item)
}

pub fn quest_tool_entries(quest: u32) -> Vec<u32> {
    embedded_azerothcore_catalog().quest_tool_entries(quest)
}

pub fn nearest_quest_tool(quest: u32, map: u32, from: Vec3) -> Option<Vec3> {
    embedded_azerothcore_catalog().nearest_quest_tool(quest, map, from)
}

pub fn observed_control_spells_for_target(entry: u32) -> &'static [u32] {
    embedded_azerothcore_catalog().observed_control_spells_for_target(entry)
}

pub fn quest_spell_rule(
    quest: u32,
    kind: QuestTargetKind,
    entry: u32,
) -> Option<&'static QuestSpellRule> {
    embedded_azerothcore_catalog().quest_spell_rule(quest, kind, entry)
}

pub fn quest_item_use_rule(quest: u32) -> Option<&'static QuestItemUseRule> {
    embedded_azerothcore_catalog().quest_item_use_rule(quest)
}

pub fn nearest_item_source(item: u32, map: u32, from: Vec3) -> Option<Vec3> {
    embedded_azerothcore_catalog().nearest_item_source(item, map, from)
}

pub fn item_source_spawns(item: u32, map: u32, from: Vec3) -> Vec<Vec3> {
    embedded_azerothcore_catalog().item_source_spawns(item, map, from)
}

pub fn nearest_turn_in(quest: u32, map: u32, from: Vec3) -> Option<Vec3> {
    embedded_azerothcore_catalog().nearest_turn_in(quest, map, from)
}

pub fn format_version() -> u32 {
    embedded_azerothcore_catalog().format_version()
}

/// Quest-control activations not exposed by the quest objective table.
pub const fn quest_tool_activation_spell(quest: u32) -> Option<u32> {
    match quest {
        12641 => Some(6247),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_world_knowledge_has_quest_data_and_rules() {
        assert_eq!(format_version(), 5);
        assert!(quest_definition(33).is_some());
        assert!(observed_control_spells_for_target(28525).contains(&51858));
        assert!(quest_tool_entries(12641).contains(&191609));
        assert_eq!(quest_tool_activation_spell(12641), Some(6247));
        let item_use = quest_item_use_rule(5441).unwrap();
        assert_eq!(
            (
                item_use.item,
                item_use.spell,
                item_use.entry,
                item_use.objective_entry,
                item_use.count
            ),
            (16114, 19938, 10556, 10556, 1)
        );
        let rule = quest_spell_rule(9283, QuestTargetKind::Creature, 16483).unwrap();
        assert_eq!(rule.name, "Gift of the Naaru");
        assert!(rule.spells.contains(&59542));
        assert!(quest_spell_rule(9283, QuestTargetKind::GameObject, 16483).is_none());
    }
}
