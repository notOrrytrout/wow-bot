use super::static_hints;
use std::collections::BTreeSet;
use wow_domain::{EntityId, Vec3, WorldPosition};
use wow_state::{
    Snapshot,
    entities::{EntityKind, EntityState},
    quests::{QuestDefinition, QuestTargetKind},
};

#[derive(Clone, Debug, PartialEq)]
pub enum ObjectiveResolution {
    GroundedCreature {
        objective: usize,
        target: EntityId,
    },
    GroundedGameObject {
        objective: usize,
        target: EntityId,
    },
    GroundedControlledSpell {
        objective: usize,
        target: EntityId,
        spell: u32,
    },
    GroundedQuestSpell {
        objective: usize,
        target: EntityId,
        spell: u32,
        name: &'static str,
    },
    QuestSpellUnavailable {
        objective: usize,
        target: EntityId,
        name: &'static str,
    },
    GroundedScriptedItemUse {
        objective: usize,
        target: EntityId,
        item: u32,
        spell: u32,
        cast_count: u8,
    },
    GroundedQuestTool {
        target: EntityId,
        activation_spell: Option<u32>,
    },
    QuestToolSearch {
        destination: Vec3,
    },
    GroundedItemCreature {
        item: u32,
        target: EntityId,
        dead: bool,
    },
    GroundedItemGameObject {
        item: u32,
        target: EntityId,
    },
    SearchArea {
        objective: usize,
        destination: Vec3,
        alternatives: Vec<Vec3>,
        source: &'static str,
    },
    ItemCollection {
        item: u32,
        required: u32,
        current: u32,
        destinations: Vec<Vec3>,
    },
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

    if let Some(rule) = static_hints::quest_item_use_rule(quest) {
        let kind: QuestTargetKind = rule.kind.into();
        if let Some(target_def) = definition
            .targets
            .iter()
            .find(|target| target.kind == kind && target.entry == rule.objective_entry)
        {
            let objective = target_def.slot;
            let current = progress
                .objectives
                .get(objective)
                .copied()
                .unwrap_or_default();
            if current < target_def.required {
                if let Some(entity) = nearest_live_target(
                    snapshot,
                    player,
                    target_def.kind,
                    rule.entry,
                    true,
                    objective,
                    excluded,
                ) {
                    return ObjectiveResolution::GroundedScriptedItemUse {
                        objective,
                        target: entity.id,
                        item: rule.item,
                        spell: rule.spell,
                        cast_count: rule.count,
                    };
                }
                if let Some(player) = player {
                    let alternatives = static_hints::target_spawns(
                        target_def.kind,
                        rule.entry,
                        player.map,
                        player.point,
                    );
                    if let Some(destination) = alternatives.first().copied() {
                        return ObjectiveResolution::SearchArea {
                            objective,
                            destination,
                            alternatives,
                            source: "scripted-item-target-spawn",
                        };
                    }
                }
            }
        }
    }

    // A live target for a grounded quest-item rule takes priority over its
    // quest-bound setup object. When the target is absent, the setup object
    // can create it. The static tool entry only guides search; interaction
    // still requires a live authoritative game object.
    if snapshot.state.control.mover.is_none() {
        let tool_entries = static_hints::quest_tool_entries(quest);
        if !tool_entries.is_empty() {
            if let Some(entity) = nearest_live_gameobject_entry(snapshot, player, &tool_entries) {
                return ObjectiveResolution::GroundedQuestTool {
                    target: entity.id,
                    activation_spell: static_hints::quest_tool_activation_spell(quest),
                };
            }
            if let Some(player) = player {
                if let Some(destination) =
                    static_hints::nearest_quest_tool(quest, player.map, player.point)
                {
                    return ObjectiveResolution::QuestToolSearch { destination };
                }
            }
        }
    }

    for target in &definition.targets {
        let index = target.slot;
        let current = progress.objectives.get(index).copied().unwrap_or_default();
        if current >= target.required {
            continue;
        }
        if let Some(entity) = nearest_live_target(
            snapshot,
            player,
            target.kind,
            target.entry,
            true,
            index,
            excluded,
        ) {
            if let Some(rule) = static_hints::quest_spell_rule(quest, target.kind, target.entry) {
                return rule
                    .spells
                    .iter()
                    .copied()
                    .find(|spell| snapshot.state.capabilities.spells.contains(spell))
                    .map(|spell| ObjectiveResolution::GroundedQuestSpell {
                        objective: index,
                        target: entity.id,
                        spell,
                        name: rule.name.as_str(),
                    })
                    .unwrap_or(ObjectiveResolution::QuestSpellUnavailable {
                        objective: index,
                        target: entity.id,
                        name: rule.name.as_str(),
                    });
            }
            if target.kind == QuestTargetKind::Creature && snapshot.state.control.mover.is_some() {
                if let Some(spell) = static_hints::observed_control_spells_for_target(target.entry)
                    .iter()
                    .copied()
                    .find(|spell| snapshot.state.control.abilities.contains(spell))
                {
                    return ObjectiveResolution::GroundedControlledSpell {
                        objective: index,
                        target: entity.id,
                        spell,
                    };
                }
            }
            return match target.kind {
                QuestTargetKind::Creature => ObjectiveResolution::GroundedCreature {
                    objective: index,
                    target: entity.id,
                },
                QuestTargetKind::GameObject => ObjectiveResolution::GroundedGameObject {
                    objective: index,
                    target: entity.id,
                },
            };
        }
        if let Some(destination) = poi_destination(player, definition) {
            return ObjectiveResolution::SearchArea {
                objective: index,
                destination,
                alternatives: vec![destination],
                source: "server-poi",
            };
        }
        if let Some(player) = player {
            let alternatives =
                static_hints::target_spawns(target.kind, target.entry, player.map, player.point);
            if let Some(destination) = alternatives.first().copied() {
                return ObjectiveResolution::SearchArea {
                    objective: index,
                    destination,
                    alternatives,
                    source: "azerothcore-static-spawn",
                };
            }
        }
    }

    for item in &definition.items {
        let current = snapshot.state.inventory.count(item.item);
        if current < item.required {
            let source_entries = static_hints::item_source_entries(item.item);
            for (kind, entry) in &source_entries {
                if let Some(entity) = nearest_live_target(
                    snapshot,
                    player,
                    *kind,
                    *entry,
                    false,
                    usize::MAX,
                    &BTreeSet::new(),
                ) {
                    return match kind {
                        QuestTargetKind::Creature => ObjectiveResolution::GroundedItemCreature {
                            item: item.item,
                            target: entity.id,
                            dead: entity.is_dead(),
                        },
                        QuestTargetKind::GameObject => {
                            ObjectiveResolution::GroundedItemGameObject {
                                item: item.item,
                                target: entity.id,
                            }
                        }
                    };
                }
            }
            let destinations = player
                .map(|player| static_hints::item_source_spawns(item.item, player.map, player.point))
                .unwrap_or_default();
            return ObjectiveResolution::ItemCollection {
                item: item.item,
                required: item.required,
                current,
                destinations,
            };
        }
    }
    ObjectiveResolution::NoSupportedObjective
}

fn nearest_live_gameobject_entry<'a>(
    snapshot: &'a Snapshot,
    player: Option<WorldPosition>,
    entries: &[u32],
) -> Option<&'a EntityState> {
    nearest_live_entity(snapshot, player, |entity| {
        entity.kind == EntityKind::GameObject && entries.contains(&entity.entry)
    })
}

fn nearest_live_target<'a>(
    snapshot: &'a Snapshot,
    player: Option<WorldPosition>,
    kind: QuestTargetKind,
    entry: u32,
    require_alive: bool,
    objective: usize,
    excluded: &BTreeSet<(usize, EntityId)>,
) -> Option<&'a EntityState> {
    nearest_live_entity(snapshot, player, |entity| {
        entity.entry == entry
            && match kind {
                QuestTargetKind::Creature => entity.kind == EntityKind::Unit,
                QuestTargetKind::GameObject => entity.kind == EntityKind::GameObject,
            }
            && !excluded.contains(&(objective, entity.id))
            && (!require_alive || !entity.is_dead())
    })
}

fn nearest_live_entity(
    snapshot: &Snapshot,
    player: Option<WorldPosition>,
    matches: impl Fn(&EntityState) -> bool,
) -> Option<&EntityState> {
    let player = player?;
    snapshot
        .state
        .entities
        .0
        .values()
        .filter(|entity| matches(entity))
        .filter_map(|entity| {
            let position = entity.position?;
            (position.map == player.map).then_some((player.point.distance(position.point), entity))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, entity)| entity)
}

fn poi_destination(player: Option<WorldPosition>, definition: &QuestDefinition) -> Option<Vec3> {
    let player = player?;
    let map = definition.poi_map?;
    if map != player.map {
        return None;
    }
    let x = definition.poi_x?;
    let y = definition.poi_y?;
    let destination = Vec3::new(x, y, player.point.z);
    destination.is_finite().then_some(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{EntityId, WorldPosition};
    use wow_state::{
        AuthoritativeState,
        entities::{EntityKind, EntityState},
        quests::{QuestDefinition, QuestProgress, QuestTargetObjective},
    };

    #[test]
    fn live_target_wins_over_poi_hint() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.quests.active.insert(
            1,
            QuestProgress {
                complete: false,
                objectives: vec![0, 0, 0, 0],
            },
        );
        state.quests.definitions.insert(
            1,
            QuestDefinition {
                quest: 1,
                title: "x".into(),
                poi_map: Some(0),
                poi_x: Some(100.0),
                poi_y: Some(100.0),
                targets: vec![QuestTargetObjective {
                    slot: 0,
                    kind: QuestTargetKind::Creature,
                    entry: 7,
                    required: 1,
                    item_drop: 0,
                    text: String::new(),
                }],
                items: vec![],
            },
        );
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                entry: 7,
                kind: EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 0,
                    point: Vec3::new(10.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(
            resolve(&snapshot, 1),
            ObjectiveResolution::GroundedCreature {
                objective: 0,
                target: EntityId(9)
            }
        );
    }
    #[test]
    fn controlled_eye_uses_observed_siphon_instead_of_attack() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition {
            map: 609,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.control.mover = Some(EntityId(500));
        state.control.mover_position = Some(WorldPosition {
            map: 609,
            point: Vec3::new(1.0, 0.0, 10.0),
            orientation: 0.0,
        });
        state.control.abilities.insert(51858);
        state.quests.active.insert(
            12641,
            QuestProgress {
                complete: false,
                objectives: vec![0, 0, 0, 0],
            },
        );
        state.quests.definitions.insert(
            12641,
            QuestDefinition {
                quest: 12641,
                title: "Death Comes From On High".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: vec![QuestTargetObjective {
                    slot: 0,
                    kind: QuestTargetKind::Creature,
                    entry: 28525,
                    required: 1,
                    item_drop: 0,
                    text: "Forge analyzed".into(),
                }],
                items: vec![],
            },
        );
        state.entities.0.insert(
            EntityId(900),
            EntityState {
                id: EntityId(900),
                entry: 28525,
                kind: EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 609,
                    point: Vec3::new(5.0, 0.0, 10.0),
                    orientation: 0.0,
                }),
                ..Default::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(
            resolve(&snapshot, 12641),
            ObjectiveResolution::GroundedControlledSpell {
                objective: 0,
                target: EntityId(900),
                spell: 51858
            }
        );
    }

    #[test]
    fn survivor_requires_observed_gift_of_the_naaru_instead_of_attack() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition {
            map: 530,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.quests.active.insert(
            9283,
            QuestProgress {
                complete: false,
                objectives: vec![0, 0, 0, 0],
            },
        );
        state.quests.definitions.insert(
            9283,
            QuestDefinition {
                quest: 9283,
                title: "Rescue the Survivors!".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: vec![QuestTargetObjective {
                    slot: 0,
                    kind: QuestTargetKind::Creature,
                    entry: 16483,
                    required: 1,
                    item_drop: 0,
                    text: "Survivors healed".into(),
                }],
                items: vec![],
            },
        );
        state.entities.0.insert(
            EntityId(42),
            EntityState {
                id: EntityId(42),
                entry: 16483,
                kind: EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 530,
                    point: Vec3::new(5.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                health: Some((50, 100)),
                ..Default::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);

        assert_eq!(
            resolve(&snapshot, 9283),
            ObjectiveResolution::QuestSpellUnavailable {
                objective: 0,
                target: EntityId(42),
                name: "Gift of the Naaru"
            }
        );
        state.capabilities.spells.insert(59542);
        assert_eq!(
            resolve(&Snapshot::from_state(&state), 9283),
            ObjectiveResolution::GroundedQuestSpell {
                objective: 0,
                target: EntityId(42),
                spell: 59542,
                name: "Gift of the Naaru"
            }
        );
    }

    #[test]
    fn lazy_peons_excludes_already_credited_target_guid() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.quests.active.insert(
            5441,
            QuestProgress {
                complete: false,
                objectives: vec![1, 0, 0, 0],
            },
        );
        state.quests.definitions.insert(
            5441,
            QuestDefinition {
                quest: 5441,
                title: "Lazy Peons".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: vec![QuestTargetObjective {
                    slot: 0,
                    kind: QuestTargetKind::Creature,
                    entry: 10556,
                    required: 5,
                    item_drop: 0,
                    text: "Lazy Peons awakened".into(),
                }],
                items: vec![],
            },
        );
        state.entities.0.insert(
            EntityId(55),
            EntityState {
                id: EntityId(55),
                entry: 10556,
                kind: EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 1,
                    point: Vec3::new(2.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        state.entities.0.insert(
            EntityId(56),
            EntityState {
                id: EntityId(56),
                entry: 10556,
                kind: EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 1,
                    point: Vec3::new(4.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        let mut excluded = BTreeSet::new();
        excluded.insert((0, EntityId(55)));
        assert_eq!(
            resolve_with_exclusions(&snapshot, 5441, &excluded),
            ObjectiveResolution::GroundedScriptedItemUse {
                objective: 0,
                target: EntityId(56),
                item: 16114,
                spell: 19938,
                cast_count: 1
            }
        );
    }

    #[test]
    fn item_objective_uses_authoritative_inventory_count() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.quests.active.insert(
            2,
            QuestProgress {
                complete: false,
                objectives: vec![0, 0, 0, 0],
            },
        );
        state.quests.definitions.insert(
            2,
            QuestDefinition {
                quest: 2,
                title: "item quest".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: vec![],
                items: vec![wow_state::quests::QuestItemObjective {
                    item: 16305,
                    required: 1,
                }],
            },
        );
        state.inventory.items.insert(16305, 1);
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(
            resolve(&snapshot, 2),
            ObjectiveResolution::NoSupportedObjective
        );
    }

    #[test]
    fn lazy_peons_resolves_to_scripted_item_use_not_attack() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition {
            map: 1,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.quests.active.insert(
            5441,
            QuestProgress {
                complete: false,
                objectives: vec![0, 0, 0, 0],
            },
        );
        state.quests.definitions.insert(
            5441,
            QuestDefinition {
                quest: 5441,
                title: "Lazy Peons".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: vec![QuestTargetObjective {
                    slot: 0,
                    kind: QuestTargetKind::Creature,
                    entry: 10556,
                    required: 5,
                    item_drop: 0,
                    text: "Lazy Peons awakened".into(),
                }],
                items: vec![],
            },
        );
        state.entities.0.insert(
            EntityId(55),
            EntityState {
                id: EntityId(55),
                entry: 10556,
                kind: EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 1,
                    point: Vec3::new(3.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        let snapshot = Snapshot::from_state(&state);
        assert_eq!(
            resolve(&snapshot, 5441),
            ObjectiveResolution::GroundedScriptedItemUse {
                objective: 0,
                target: EntityId(55),
                item: 16114,
                spell: 19938,
                cast_count: 1
            }
        );
    }

    #[test]
    fn scripted_temporary_creature_item_target_resolves_only_from_live_state() {
        let mut state = AuthoritativeState::default();
        state.position.player = Some(WorldPosition {
            map: 530,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.quests.active.insert(
            10584,
            QuestProgress {
                complete: false,
                objectives: vec![0, 0, 0, 0],
            },
        );
        state.quests.definitions.insert(
            10584,
            QuestDefinition {
                quest: 10584,
                title: "Picking Up Some Power Converters".into(),
                poi_map: None,
                poi_x: None,
                poi_y: None,
                targets: vec![QuestTargetObjective {
                    slot: 0,
                    kind: QuestTargetKind::Creature,
                    entry: 21731,
                    required: 5,
                    item_drop: 0,
                    text: "Electromentals collected".into(),
                }],
                items: vec![],
            },
        );

        let absent_target = Snapshot::from_state(&state);
        assert!(matches!(
            resolve(&absent_target, 10584),
            ObjectiveResolution::QuestToolSearch { .. }
        ));

        state.entities.0.insert(
            EntityId(10584),
            EntityState {
                id: EntityId(10584),
                entry: 21729,
                kind: EntityKind::Unit,
                position: Some(WorldPosition {
                    map: 530,
                    point: Vec3::new(3.0, 0.0, 0.0),
                    orientation: 0.0,
                }),
                health: Some((100, 100)),
                ..Default::default()
            },
        );
        let live_target = Snapshot::from_state(&state);
        assert_eq!(
            resolve(&live_target, 10584),
            ObjectiveResolution::GroundedScriptedItemUse {
                objective: 0,
                target: EntityId(10584),
                item: 30656,
                spell: 37136,
                cast_count: 1,
            }
        );
    }
}
