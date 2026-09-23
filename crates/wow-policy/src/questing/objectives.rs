use std::collections::BTreeSet;
use wow_domain::{EntityId, Vec3};
use wow_state::{quests::{QuestDefinition, QuestTargetKind}, Snapshot};
use super::static_hints;

#[derive(Clone, Debug, PartialEq)]
pub enum ObjectiveResolution {
    GroundedCreature { objective: usize, target: EntityId },
    GroundedGameObject { objective: usize, target: EntityId },
    GroundedControlledSpell { objective: usize, target: EntityId, spell: u32 },
    GroundedScriptedItemUse { objective: usize, target: EntityId, item: u32, spell: u32, cast_count: u8 },
    GroundedQuestTool { target: EntityId, activation_spell: Option<u32> },
    QuestToolSearch { destination: Vec3 },
    GroundedItemCreature { item: u32, target: EntityId, dead: bool },
    GroundedItemGameObject { item: u32, target: EntityId },
    SearchArea { objective: usize, destination: Vec3, source: &'static str },
    ItemCollection { item: u32, required: u32, current: u32, destination: Option<Vec3> },
    WaitingForDefinition,
    NoSupportedObjective,
}

pub fn resolve(snapshot: &Snapshot, quest: u32) -> ObjectiveResolution {
    resolve_with_exclusions(snapshot, quest, &BTreeSet::new())
}

pub fn resolve_with_exclusions(
    snapshot: &Snapshot,
    quest: u32,
    excluded: &BTreeSet<(usize, EntityId)>,
) -> ObjectiveResolution {
    let Some(progress) = snapshot.state.quests.active.get(&quest) else {
        return ObjectiveResolution::NoSupportedObjective;
    };
    let Some(definition) = snapshot.state.quests.definitions.get(&quest) else {
        return ObjectiveResolution::WaitingForDefinition;
    };
    let player = snapshot.state.position.player;

    // Some quests require activating a quest-bound control object before their
    // actual objectives are actionable (vehicles, possession, scripted tools).
    // AzerothCore goober.questId is static search guidance; a live game object
    // is still required before interaction.
    if snapshot.state.control.mover.is_none() {
        let tool_entries = static_hints::quest_tool_entries(quest);
        if !tool_entries.is_empty() {
            if let Some(entity) = nearest_live_gameobject_entry(snapshot, &tool_entries) {
                return ObjectiveResolution::GroundedQuestTool { target: entity.id, activation_spell: static_hints::quest_tool_activation_spell(quest) };
            }
            if let Some(player) = player {
                if let Some(destination) = static_hints::nearest_quest_tool(quest, player.map, player.point) {
                    return ObjectiveResolution::QuestToolSearch { destination };
                }
            }
        }
    }

    if let Some((item, spell, target_entry, cast_count)) = static_hints::scripted_item_use(quest) {
        if let Some(target_def) = definition.targets.iter().find(|target| target.entry == target_entry) {
            let objective = target_def.slot;
            let current = progress.objectives.get(objective).copied().unwrap_or_default();
            if current < target_def.required {
                if let Some(entity) = nearest_live_target(snapshot, QuestTargetKind::Creature, target_entry, true, objective, excluded) {
                    return ObjectiveResolution::GroundedScriptedItemUse { objective, target: entity.id, item, spell, cast_count };
                }
                if let Some(player) = player {
                    if let Some(destination) = static_hints::nearest_target_spawn(QuestTargetKind::Creature, target_entry, player.map, player.point) {
                        return ObjectiveResolution::SearchArea { objective, destination, source: "scripted-item-target-spawn" };
                    }
                }
            }
        }
    }

    for target in &definition.targets {
        let index = target.slot;
        let current = progress.objectives.get(index).copied().unwrap_or_default();
        if current >= target.required { continue; }
        if let Some(entity) = nearest_live_target(snapshot, target.kind, target.entry, true, index, excluded) {
            if target.kind == QuestTargetKind::Creature && snapshot.state.control.mover.is_some() {
                if let Some(spell) = static_hints::observed_control_spells_for_target(target.entry)
                    .iter().copied().find(|spell| snapshot.state.control.abilities.contains(spell))
                {
                    return ObjectiveResolution::GroundedControlledSpell { objective: index, target: entity.id, spell };
                }
            }
            return match target.kind {
                QuestTargetKind::Creature => ObjectiveResolution::GroundedCreature { objective: index, target: entity.id },
                QuestTargetKind::GameObject => ObjectiveResolution::GroundedGameObject { objective: index, target: entity.id },
            };
        }
        if let Some(destination) = poi_destination(snapshot, definition) {
            return ObjectiveResolution::SearchArea { objective: index, destination, source: "server-poi" };
        }
        if let Some(player) = player {
            if let Some(destination) = static_hints::nearest_target_spawn(target.kind, target.entry, player.map, player.point) {
                return ObjectiveResolution::SearchArea { objective: index, destination, source: "azerothcore-static-spawn" };
            }
        }
    }

    for item in &definition.items {
        let current = snapshot.state.inventory.items.get(&item.item).copied().unwrap_or_default();
        if current < item.required {
            let source_entries = static_hints::item_source_entries(item.item);
            for (kind, entry) in &source_entries {
                if let Some(entity) = nearest_live_target(snapshot, *kind, *entry, false, usize::MAX, &BTreeSet::new()) {
                    return match kind {
                        QuestTargetKind::Creature => ObjectiveResolution::GroundedItemCreature {
                            item: item.item,
                            target: entity.id,
                            dead: entity.health.is_some_and(|(current, _)| current == 0),
                        },
                        QuestTargetKind::GameObject => ObjectiveResolution::GroundedItemGameObject { item: item.item, target: entity.id },
                    };
                }
            }
            let destination = player.and_then(|player| static_hints::nearest_item_source(item.item, player.map, player.point));
            return ObjectiveResolution::ItemCollection { item: item.item, required: item.required, current, destination };
        }
    }
    ObjectiveResolution::NoSupportedObjective
}

fn nearest_live_gameobject_entry<'a>(snapshot: &'a Snapshot, entries: &[u32]) -> Option<&'a wow_state::entities::EntityState> {
    let player = snapshot.state.position.player?;
    snapshot.state.entities.0.values()
        .filter(|entity| entity.kind == wow_state::entities::EntityKind::GameObject && entries.contains(&entity.entry))
        .filter_map(|entity| {
            let position = entity.position?;
            (position.map == player.map).then_some((player.point.distance(position.point), entity))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, entity)| entity)
}

fn nearest_live_target<'a>(
    snapshot: &'a Snapshot,
    kind: QuestTargetKind,
    entry: u32,
    require_alive: bool,
    objective: usize,
    excluded: &BTreeSet<(usize, EntityId)>,
) -> Option<&'a wow_state::entities::EntityState> {
    let player = snapshot.state.position.player?;
    snapshot.state.entities.0.values()
        .filter(|entity| entity.entry == entry)
        .filter(|entity| match kind {
            QuestTargetKind::Creature => entity.kind == wow_state::entities::EntityKind::Unit,
            QuestTargetKind::GameObject => entity.kind == wow_state::entities::EntityKind::GameObject,
        })
        .filter(|entity| !excluded.contains(&(objective, entity.id)))
        .filter(|entity| !require_alive || !entity.health.is_some_and(|(current, _)| current == 0))
        .filter_map(|entity| {
            let position = entity.position?;
            (position.map == player.map).then_some((player.point.distance(position.point), entity))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, entity)| entity)
}

fn poi_destination(snapshot: &Snapshot, definition: &QuestDefinition) -> Option<Vec3> {
    let player = snapshot.state.position.player?;
    let map = definition.poi_map?;
    if map != player.map { return None; }
    let x = definition.poi_x?; let y = definition.poi_y?;
    let destination = Vec3::new(x, y, player.point.z);
    destination.is_finite().then_some(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{EntityId, WorldPosition};
    use wow_state::{AuthoritativeState, entities::{EntityKind, EntityState}, quests::{QuestDefinition, QuestProgress, QuestTargetObjective}};

    #[test]
    fn live_target_wins_over_poi_hint() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition { map: 0, point: Vec3::new(0.0,0.0,0.0), orientation: 0.0 });
        state.quests.active.insert(1, QuestProgress { complete: false, objectives: vec![0,0,0,0] });
        state.quests.definitions.insert(1, QuestDefinition {
            quest: 1, title: "x".into(), poi_map: Some(0), poi_x: Some(100.0), poi_y: Some(100.0),
            targets: vec![QuestTargetObjective { slot: 0, kind: QuestTargetKind::Creature, entry: 7, required: 1, item_drop: 0, text: String::new() }], items: vec![],
        });
        state.entities.0.insert(EntityId(9), EntityState { id: EntityId(9), entry: 7, kind: EntityKind::Unit, position: Some(WorldPosition { map: 0, point: Vec3::new(10.0,0.0,0.0), orientation: 0.0 }), ..Default::default() });
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(resolve(&snapshot, 1), ObjectiveResolution::GroundedCreature { objective: 0, target: EntityId(9) });
    }
    #[test]
    fn controlled_eye_uses_observed_siphon_instead_of_attack() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition { map: 609, point: Vec3::new(0.0,0.0,0.0), orientation: 0.0 });
        state.control.mover = Some(EntityId(500));
        state.control.mover_position = Some(WorldPosition { map: 609, point: Vec3::new(1.0,0.0,10.0), orientation: 0.0 });
        state.control.abilities.insert(51858);
        state.quests.active.insert(12641, QuestProgress { complete: false, objectives: vec![0,0,0,0] });
        state.quests.definitions.insert(12641, QuestDefinition {
            quest: 12641, title: "Death Comes From On High".into(), poi_map: None, poi_x: None, poi_y: None,
            targets: vec![QuestTargetObjective { slot: 0, kind: QuestTargetKind::Creature, entry: 28525, required: 1, item_drop: 0, text: "Forge analyzed".into() }], items: vec![],
        });
        state.entities.0.insert(EntityId(900), EntityState { id: EntityId(900), entry: 28525, kind: EntityKind::Unit, position: Some(WorldPosition { map: 609, point: Vec3::new(5.0,0.0,10.0), orientation: 0.0 }), ..Default::default() });
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(resolve(&snapshot, 12641), ObjectiveResolution::GroundedControlledSpell { objective: 0, target: EntityId(900), spell: 51858 });
    }


    #[test]
    fn lazy_peons_excludes_already_credited_target_guid() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition { map: 1, point: Vec3::new(0.0,0.0,0.0), orientation: 0.0 });
        state.quests.active.insert(5441, QuestProgress { complete: false, objectives: vec![1,0,0,0] });
        state.quests.definitions.insert(5441, QuestDefinition {
            quest: 5441, title: "Lazy Peons".into(), poi_map: None, poi_x: None, poi_y: None,
            targets: vec![QuestTargetObjective { slot: 0, kind: QuestTargetKind::Creature, entry: 10556, required: 5, item_drop: 0, text: "Lazy Peons awakened".into() }], items: vec![],
        });
        state.entities.0.insert(EntityId(55), EntityState { id: EntityId(55), entry: 10556, kind: EntityKind::Unit, position: Some(WorldPosition { map: 1, point: Vec3::new(2.0,0.0,0.0), orientation: 0.0 }), health: Some((100,100)), ..Default::default() });
        state.entities.0.insert(EntityId(56), EntityState { id: EntityId(56), entry: 10556, kind: EntityKind::Unit, position: Some(WorldPosition { map: 1, point: Vec3::new(4.0,0.0,0.0), orientation: 0.0 }), health: Some((100,100)), ..Default::default() });
        let snapshot = Snapshot::from_state(&state);
        let mut excluded = BTreeSet::new();
        excluded.insert((0, EntityId(55)));
        assert_eq!(
            resolve_with_exclusions(&snapshot, 5441, &excluded),
            ObjectiveResolution::GroundedScriptedItemUse { objective: 0, target: EntityId(56), item: 16114, spell: 19938, cast_count: 1 }
        );
    }

    #[test]
    fn item_objective_uses_authoritative_inventory_count() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition { map: 0, point: Vec3::new(0.0,0.0,0.0), orientation: 0.0 });
        state.quests.active.insert(2, QuestProgress { complete: false, objectives: vec![0,0,0,0] });
        state.quests.definitions.insert(2, QuestDefinition {
            quest: 2, title: "item quest".into(), poi_map: None, poi_x: None, poi_y: None, targets: vec![],
            items: vec![wow_state::quests::QuestItemObjective { item: 16305, required: 1 }],
        });
        state.inventory.items.insert(16305, 1);
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(resolve(&snapshot, 2), ObjectiveResolution::NoSupportedObjective);
    }

    #[test]
    fn lazy_peons_resolves_to_scripted_item_use_not_attack() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition { map: 1, point: Vec3::new(0.0,0.0,0.0), orientation: 0.0 });
        state.quests.active.insert(5441, QuestProgress { complete: false, objectives: vec![0,0,0,0] });
        state.quests.definitions.insert(5441, QuestDefinition {
            quest: 5441, title: "Lazy Peons".into(), poi_map: None, poi_x: None, poi_y: None,
            targets: vec![QuestTargetObjective { slot: 0, kind: QuestTargetKind::Creature, entry: 10556, required: 5, item_drop: 0, text: "Lazy Peons awakened".into() }], items: vec![],
        });
        state.entities.0.insert(EntityId(55), EntityState { id: EntityId(55), entry: 10556, kind: EntityKind::Unit, position: Some(WorldPosition { map: 1, point: Vec3::new(3.0,0.0,0.0), orientation: 0.0 }), health: Some((100,100)), ..Default::default() });
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(resolve(&snapshot, 5441), ObjectiveResolution::GroundedScriptedItemUse { objective: 0, target: EntityId(55), item: 16114, spell: 19938, cast_count: 1 });
    }

}
