use super::loader::WorldKnowledge;
pub fn validate(k: &WorldKnowledge) -> Result<(), String> {
    if k.version.trim().is_empty() {
        Err("world knowledge version is empty".into())
    } else {
        Ok(())
    }
}

use super::azerothcore::AzerothCoreCatalog;

pub fn validate_azerothcore_catalog(catalog: &AzerothCoreCatalog) -> Result<(), String> {
    if catalog.catalog_format_version != 1 {
        return Err(format!(
            "unsupported World Knowledge catalog version {}",
            catalog.catalog_format_version
        ));
    }
    if catalog.format_version < 5 {
        return Err(format!(
            "quest knowledge format {} is older than required format 5",
            catalog.format_version
        ));
    }
    if catalog.quests.is_empty() {
        return Err("quest knowledge is empty".into());
    }
    if catalog.source.project.trim().is_empty()
        || catalog.source.quest_spell_rule_generator.trim().is_empty()
        || catalog.provenance.project.trim().is_empty()
        || catalog.provenance.world_data.project.trim().is_empty()
        || catalog
            .provenance
            .world_data
            .source_layout
            .trim()
            .is_empty()
        || catalog
            .provenance
            .world_data
            .generator
            .name
            .trim()
            .is_empty()
        || catalog.provenance.world_data.generator.schema_version == 0
    {
        return Err("World Knowledge provenance is incomplete".into());
    }
    if catalog
        .source
        .quest_spell_inputs
        .values()
        .any(|hash| !is_sha256(hash))
        || catalog
            .provenance
            .world_data
            .files
            .values()
            .any(|hash| !is_sha256(hash))
        || !is_sha256(&catalog.provenance.world_data.gather_lock_source.sha256)
        || catalog
            .provenance
            .world_data
            .trainer_skill_sources
            .values()
            .any(|hash| !is_sha256(hash))
    {
        return Err("World Knowledge provenance contains an invalid source hash".into());
    }
    for (quest, definition) in &catalog.quests {
        if *quest == 0 {
            return Err(format!("quest {quest} has an invalid ID"));
        }
        if definition
            .targets
            .iter()
            .any(|target| target.entry == 0 || target.required == 0)
            || definition
                .items
                .iter()
                .any(|item| item.item == 0 || item.required == 0)
        {
            return Err(format!(
                "quest {quest} contains a target or item with a zero ID/count"
            ));
        }
    }
    for (quest, rules) in &catalog.quest_spell_rules {
        for rule in rules {
            if *quest == 0 || rule.entry == 0 || rule.spells.is_empty() || rule.spells.contains(&0)
            {
                return Err(format!("quest {quest} has an invalid quest spell rule"));
            }
            if !catalog.quests.get(quest).is_some_and(|definition| {
                definition
                    .targets
                    .iter()
                    .any(|target| target.kind == rule.kind && target.entry == rule.entry)
            }) {
                return Err(format!(
                    "quest {quest} spell rule does not match a quest objective"
                ));
            }
        }
    }
    for (quest, rule) in &catalog.quest_item_use_rules {
        if *quest == 0
            || rule.entry == 0
            || rule.objective_entry == 0
            || rule.item == 0
            || rule.spell == 0
            || rule.count == 0
        {
            return Err(format!("quest {quest} has an invalid quest item-use rule"));
        }
        if !catalog.quests.get(quest).is_some_and(|definition| {
            definition
                .targets
                .iter()
                .any(|target| target.kind == rule.kind && target.entry == rule.objective_entry)
        }) {
            return Err(format!(
                "quest {quest} item-use rule does not match a quest objective"
            ));
        }
    }
    if catalog
        .item_sources
        .iter()
        .any(|(item, sources)| *item == 0 || sources.iter().any(|(_, entry)| *entry == 0))
    {
        return Err("quest item source catalog contains a zero item or source entry".into());
    }
    for spawns in catalog
        .creature_spawns
        .values()
        .chain(catalog.gameobject_spawns.values())
    {
        if spawns
            .iter()
            .any(|spawn| !spawn.iter().all(|value| value.is_finite()) || spawn[0] < 0.0)
        {
            return Err(
                "quest spawn catalog contains a non-finite coordinate or invalid map ID".into(),
            );
        }
    }
    for service in &catalog.world.vendor_services {
        if service.entry_id == 0 || service.spawns.iter().any(|spawn| !valid_spawn(spawn)) {
            return Err("vendor service catalog contains an invalid entry or spawn".into());
        }
    }
    for start in &catalog.world.quest_starts {
        if start.quest_id == 0
            || start.giver_entry_id == 0
            || start.spawns.iter().any(|spawn| !valid_spawn(spawn))
        {
            return Err("quest starter catalog contains an invalid quest, giver, or spawn".into());
        }
    }
    for turn_in in &catalog.world.quest_turn_ins {
        if turn_in.quest_id == 0
            || turn_in.giver_entry_id == 0
            || turn_in.spawns.iter().any(|spawn| !valid_spawn(spawn))
        {
            return Err("quest turn-in catalog contains an invalid quest, giver, or spawn".into());
        }
    }
    for entries in &catalog.world.creature_spawns {
        if entries.entry_id == 0 || entries.spawns.iter().any(|spawn| !valid_spawn(spawn)) {
            return Err("creature spawn catalog contains an invalid entry or spawn".into());
        }
    }
    for entries in &catalog.world.gameobject_spawns {
        if entries.entry_id == 0 || entries.spawns.iter().any(|spawn| !valid_spawn(spawn)) {
            return Err("game-object spawn catalog contains an invalid entry or spawn".into());
        }
    }
    for source in &catalog.world.quest_item_creature_sources {
        if source.item_id == 0 || source.creature_entries.contains(&0) {
            return Err("creature item sources contain a zero item or creature entry".into());
        }
    }
    for source in &catalog.world.quest_item_gameobject_sources {
        if source.item_id == 0 || source.gameobject_entries.contains(&0) {
            return Err("game-object item sources contain a zero item or entry".into());
        }
    }
    for trainer in &catalog.world.trainer_services {
        if trainer.entry_id == 0 || trainer.spawns.iter().any(|spawn| !valid_spawn(spawn)) {
            return Err("trainer service catalog contains an invalid entry or spawn".into());
        }
    }
    for node in &catalog.world.gather_nodes {
        if node.entry_id == 0 || node.spawns.iter().any(|spawn| !valid_spawn(spawn)) {
            return Err("gather node catalog contains an invalid entry or spawn".into());
        }
    }
    Ok(())
}

fn valid_spawn(spawn: &super::azerothcore::KnowledgeSpawn) -> bool {
    spawn.x.is_finite()
        && spawn.y.is_finite()
        && spawn.z.is_finite()
        && spawn.orientation.is_finite()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
