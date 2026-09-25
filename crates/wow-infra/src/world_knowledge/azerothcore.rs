use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path, sync::OnceLock};
use wow_domain::Vec3;
use wow_state::quests::QuestTargetKind;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeEntityKind {
    Creature,
    #[serde(rename = "gameobject", alias = "game_object")]
    GameObject,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestCatalogSource {
    pub project: String,
    pub snapshot: Option<String>,
    pub quest_spell_rule_generator: String,
    pub quest_spell_inputs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GeneratorSource {
    pub name: String,
    pub schema_version: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HashedSourceFile {
    pub name: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorldDataSource {
    pub project: String,
    pub source_layout: String,
    pub azerothcore_commit: Option<String>,
    pub generator: GeneratorSource,
    pub files: BTreeMap<String, String>,
    pub gather_lock_source: HashedSourceFile,
    pub trainer_skill_sources: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CatalogProvenance {
    pub project: String,
    pub azerothcore_commit: Option<String>,
    pub quest_catalog: QuestCatalogSource,
    pub world_data: WorldDataSource,
    pub note: String,
}

impl From<KnowledgeEntityKind> for QuestTargetKind {
    fn from(value: KnowledgeEntityKind) -> Self {
        match value {
            KnowledgeEntityKind::Creature => Self::Creature,
            KnowledgeEntityKind::GameObject => Self::GameObject,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AzerothCoreCatalog {
    pub(crate) catalog_format_version: u32,
    pub(crate) format_version: u32,
    pub source: QuestCatalogSource,
    pub provenance: CatalogProvenance,
    pub(crate) quests: BTreeMap<u32, QuestKnowledge>,
    #[serde(default)]
    creature_names: BTreeMap<u32, Option<String>>,
    #[serde(default)]
    gameobject_names: BTreeMap<u32, Option<String>>,
    pub(crate) quest_ends: BTreeMap<u32, Vec<(KnowledgeEntityKind, u32)>>,
    pub(crate) creature_spawns: BTreeMap<u32, Vec<[f32; 5]>>,
    pub(crate) gameobject_spawns: BTreeMap<u32, Vec<[f32; 5]>>,
    pub(crate) item_sources: BTreeMap<u32, Vec<(KnowledgeEntityKind, u32)>>,
    #[serde(default)]
    pub(crate) spell_targets: BTreeMap<u32, Vec<u32>>,
    #[serde(default)]
    pub(crate) quest_tools: BTreeMap<u32, Vec<QuestTool>>,
    #[serde(default)]
    pub(crate) quest_spell_rules: BTreeMap<u32, Vec<QuestSpellRule>>,
    #[serde(default)]
    pub(crate) quest_item_use_rules: BTreeMap<u32, QuestItemUseRule>,
    #[serde(default)]
    unresolved_quest_action_hints: Vec<QuestActionHintCandidate>,
    #[serde(default)]
    pub(crate) world: WorldKnowledgeData,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestKnowledge {
    pub title: String,
    pub targets: Vec<QuestTarget>,
    pub items: Vec<QuestItemRequirement>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestTarget {
    pub kind: KnowledgeEntityKind,
    pub entry: u32,
    pub required: u32,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestItemRequirement {
    pub item: u32,
    pub required: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestTool {
    pub entry: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestSpellRule {
    pub kind: KnowledgeEntityKind,
    pub entry: u32,
    pub name: String,
    pub spells: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestItemUseRule {
    pub kind: KnowledgeEntityKind,
    pub entry: u32,
    pub objective_entry: u32,
    pub item: u32,
    pub spell: u32,
    pub count: u8,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestActionHintCandidate {
    pub quest: u32,
    pub title: String,
    pub reason: String,
    #[serde(default)]
    pub items: Vec<u32>,
    #[serde(default)]
    pub spell_names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorldKnowledgeData {
    #[serde(default)]
    pub format_version: u32,
    pub source: WorldDataSource,
    #[serde(default)]
    pub quest_starts: Vec<QuestStart>,
    #[serde(default)]
    pub quest_turn_ins: Vec<QuestTurnIn>,
    #[serde(default)]
    pub quest_item_creature_sources: Vec<QuestItemCreatureSource>,
    #[serde(default)]
    pub quest_item_gameobject_sources: Vec<QuestItemGameObjectSource>,
    #[serde(default)]
    pub creature_spawns: Vec<EntrySpawns>,
    #[serde(default)]
    pub gameobject_spawns: Vec<EntrySpawns>,
    #[serde(default)]
    pub vendor_services: Vec<VendorService>,
    #[serde(default)]
    pub trainer_services: Vec<TrainerService>,
    #[serde(default)]
    pub gather_nodes: Vec<GatherNode>,
    #[serde(default)]
    pub fishing_items: Vec<FishingItem>,
}

impl Default for WorldKnowledgeData {
    fn default() -> Self {
        Self {
            format_version: 0,
            source: WorldDataSource {
                project: String::new(),
                source_layout: String::new(),
                azerothcore_commit: None,
                generator: GeneratorSource {
                    name: String::new(),
                    schema_version: 0,
                },
                files: BTreeMap::new(),
                gather_lock_source: HashedSourceFile {
                    name: String::new(),
                    sha256: String::new(),
                },
                trainer_skill_sources: BTreeMap::new(),
            },
            quest_starts: Vec::new(),
            quest_turn_ins: Vec::new(),
            quest_item_creature_sources: Vec::new(),
            quest_item_gameobject_sources: Vec::new(),
            creature_spawns: Vec::new(),
            gameobject_spawns: Vec::new(),
            vendor_services: Vec::new(),
            trainer_services: Vec::new(),
            gather_nodes: Vec::new(),
            fishing_items: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestStart {
    pub quest_id: u32,
    pub giver_entry_id: u32,
    pub giver_kind: KnowledgeEntityKind,
    pub giver_name: Option<String>,
    pub allowable_classes: u32,
    pub spawns: Vec<KnowledgeSpawn>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuestStartLocation {
    pub quest_id: u32,
    pub giver_entry_id: u32,
    pub giver_kind: KnowledgeEntityKind,
    pub giver_name: Option<String>,
    pub location: Vec3,
    pub distance: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestTurnIn {
    pub quest_id: u32,
    pub giver_entry_id: u32,
    pub giver_kind: KnowledgeEntityKind,
    pub giver_name: Option<String>,
    pub spawns: Vec<KnowledgeSpawn>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestItemCreatureSource {
    pub item_id: u32,
    pub creature_entries: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestItemGameObjectSource {
    pub item_id: u32,
    pub gameobject_entries: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EntrySpawns {
    pub entry_id: u32,
    pub spawns: Vec<KnowledgeSpawn>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct KnowledgeSpawn {
    pub map_id: u32,
    pub zone_id: u32,
    pub area_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VendorService {
    pub entry_id: u32,
    pub can_sell: bool,
    pub can_repair: bool,
    pub can_auction: bool,
    pub spawns: Vec<KnowledgeSpawn>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TrainerService {
    pub entry_id: u32,
    pub trainer_id: u32,
    pub name: Option<String>,
    pub skills: Vec<u32>,
    pub spawns: Vec<KnowledgeSpawn>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GatherNode {
    pub entry_id: u32,
    pub name: String,
    pub kind: GatheringKind,
    pub required_skill: u32,
    pub spawns: Vec<KnowledgeSpawn>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GatheringKind {
    Mining,
    Herbalism,
    Fishing,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FishingItem {
    pub item_id: u32,
    pub name: String,
}

impl AzerothCoreCatalog {
    pub fn catalog_format_version(&self) -> u32 {
        self.catalog_format_version
    }
    pub fn format_version(&self) -> u32 {
        self.format_version
    }
    pub fn world(&self) -> &WorldKnowledgeData {
        &self.world
    }
    /// Return quest giver locations on `map` within `max_distance`, nearest first.
    /// `class_id` uses 1-based WoW class IDs; unknown classes can use only
    /// quests with no class restriction.
    pub fn nearby_quest_starts(
        &self,
        map: u32,
        from: Vec3,
        class_id: Option<u8>,
        max_distance: f32,
    ) -> Vec<QuestStartLocation> {
        nearby_quest_start_locations(&self.world.quest_starts, map, from, class_id, max_distance)
    }
    pub fn provenance(&self) -> &CatalogProvenance {
        &self.provenance
    }
    pub fn unresolved_quest_action_hints(&self) -> &[QuestActionHintCandidate] {
        &self.unresolved_quest_action_hints
    }
    pub fn quest(&self, quest_id: u32) -> Option<&QuestKnowledge> {
        self.quests.get(&quest_id)
    }
    pub fn creature_name(&self, entry: u32) -> Option<&str> {
        self.creature_names.get(&entry)?.as_deref()
    }
    pub fn gameobject_name(&self, entry: u32) -> Option<&str> {
        self.gameobject_names.get(&entry)?.as_deref()
    }
    pub fn item_source_entries(&self, item: u32) -> Vec<(QuestTargetKind, u32)> {
        self.item_sources
            .get(&item)
            .into_iter()
            .flatten()
            .map(|(kind, entry)| ((*kind).into(), *entry))
            .collect()
    }
    pub fn quest_tool_entries(&self, quest: u32) -> Vec<u32> {
        self.quest_tools
            .get(&quest)
            .into_iter()
            .flatten()
            .map(|tool| tool.entry)
            .collect()
    }
    pub fn observed_control_spells_for_target(&self, entry: u32) -> &[u32] {
        self.spell_targets
            .get(&entry)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    pub fn quest_spell_rule(
        &self,
        quest: u32,
        kind: QuestTargetKind,
        entry: u32,
    ) -> Option<&QuestSpellRule> {
        self.quest_spell_rules
            .get(&quest)?
            .iter()
            .find(|rule| QuestTargetKind::from(rule.kind) == kind && rule.entry == entry)
    }
    pub fn quest_item_use_rule(&self, quest: u32) -> Option<&QuestItemUseRule> {
        self.quest_item_use_rules.get(&quest)
    }
    pub fn nearest_spawn(
        &self,
        kind: QuestTargetKind,
        entry: u32,
        map: u32,
        from: Vec3,
    ) -> Option<Vec3> {
        self.target_spawns(kind, entry, map, from)
            .into_iter()
            .next()
    }
    pub fn target_spawns(
        &self,
        kind: QuestTargetKind,
        entry: u32,
        map: u32,
        from: Vec3,
    ) -> Vec<Vec3> {
        let spawns = match kind {
            QuestTargetKind::Creature => self.creature_spawns.get(&entry),
            QuestTargetKind::GameObject => self.gameobject_spawns.get(&entry),
        };
        ordered_spawns(spawns.into_iter().flatten(), map, from)
    }
    pub fn nearest_item_source(&self, item: u32, map: u32, from: Vec3) -> Option<Vec3> {
        self.nearest_spawn_for_entries(self.item_sources.get(&item)?.iter().copied(), map, from)
    }
    pub fn item_source_spawns(&self, item: u32, map: u32, from: Vec3) -> Vec<Vec3> {
        let points = self
            .item_sources
            .get(&item)
            .into_iter()
            .flatten()
            .flat_map(|(kind, entry)| self.target_spawns((*kind).into(), *entry, map, from));
        ordered_points(points, from)
    }
    pub fn nearest_turn_in(&self, quest: u32, map: u32, from: Vec3) -> Option<Vec3> {
        self.nearest_spawn_for_entries(
            self.quest_ends
                .get(&quest)?
                .iter()
                .map(|(kind, entry)| (*kind, *entry)),
            map,
            from,
        )
    }
    pub fn nearest_quest_tool(&self, quest: u32, map: u32, from: Vec3) -> Option<Vec3> {
        self.nearest_spawn_for_entries(
            self.quest_tools
                .get(&quest)?
                .iter()
                .map(|tool| (KnowledgeEntityKind::GameObject, tool.entry)),
            map,
            from,
        )
    }
    pub fn nearest_vendor(&self, service: VendorKind, map: u32, from: Vec3) -> Option<(u32, Vec3)> {
        closest_by_distance(
            self.world
                .vendor_services
                .iter()
                .filter(|vendor| match service {
                    VendorKind::Sell => vendor.can_sell,
                    VendorKind::Repair => vendor.can_repair,
                    VendorKind::Auction => vendor.can_auction,
                })
                .flat_map(|vendor| {
                    vendor.spawns.iter().filter_map(move |spawn| {
                        nearest_location(spawn, map)
                            .map(|point| (from.distance(point), vendor.entry_id, point))
                    })
                })
                .map(|(distance, entry, point)| (distance, (entry, point))),
        )
    }
    pub fn nearest_profession_trainer(
        &self,
        skill: u32,
        map: u32,
        from: Vec3,
    ) -> Option<(u32, Vec3)> {
        closest_by_distance(
            self.world
                .trainer_services
                .iter()
                .filter(|trainer| trainer.skills.contains(&skill))
                .flat_map(|trainer| {
                    trainer.spawns.iter().filter_map(move |spawn| {
                        nearest_location(spawn, map)
                            .map(|point| (from.distance(point), trainer.entry_id, point))
                    })
                })
                .map(|(distance, entry, point)| (distance, (entry, point))),
        )
    }
    pub fn nearest_gather_node(
        &self,
        kind: GatheringKind,
        map: u32,
        from: Vec3,
    ) -> Option<(&GatherNode, Vec3)> {
        closest_by_distance(
            self.world
                .gather_nodes
                .iter()
                .filter(|node| node.kind == kind)
                .flat_map(|node| {
                    node.spawns.iter().filter_map(move |spawn| {
                        nearest_location(spawn, map)
                            .map(|point| (from.distance(point), node, point))
                    })
                })
                .map(|(distance, node, point)| (distance, (node, point))),
        )
    }
    fn nearest_spawn_for_entries(
        &self,
        entries: impl IntoIterator<Item = (KnowledgeEntityKind, u32)>,
        map: u32,
        from: Vec3,
    ) -> Option<Vec3> {
        closest_by_distance(entries.into_iter().filter_map(|(kind, entry)| {
            self.nearest_spawn(kind.into(), entry, map, from)
                .map(|point| (from.distance(point), point))
        }))
    }
}

fn nearby_quest_start_locations(
    starts: &[QuestStart],
    map: u32,
    from: Vec3,
    class_id: Option<u8>,
    max_distance: f32,
) -> Vec<QuestStartLocation> {
    if !from.is_finite() || !max_distance.is_finite() || max_distance < 0.0 {
        return Vec::new();
    }
    let class_allowed = |mask: u32| {
        mask == 0
            || class_id
                .filter(|id| (1..=32).contains(id))
                .is_some_and(|id| mask & (1u32 << (id - 1)) != 0)
    };
    let mut locations = starts
        .iter()
        .filter(|start| class_allowed(start.allowable_classes))
        .flat_map(|start| {
            start.spawns.iter().filter_map(move |spawn| {
                if spawn.map_id != map {
                    return None;
                }
                let location = Vec3::new(spawn.x, spawn.y, spawn.z);
                let distance = from.distance(location);
                (location.is_finite() && distance.is_finite() && distance <= max_distance).then(
                    || QuestStartLocation {
                        quest_id: start.quest_id,
                        giver_entry_id: start.giver_entry_id,
                        giver_kind: start.giver_kind,
                        giver_name: start.giver_name.clone(),
                        location,
                        distance,
                    },
                )
            })
        })
        .collect::<Vec<_>>();
    locations.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.quest_id.cmp(&b.quest_id))
            .then_with(|| a.giver_entry_id.cmp(&b.giver_entry_id))
    });
    locations
}

fn closest_by_distance<T>(candidates: impl IntoIterator<Item = (f32, T)>) -> Option<T> {
    candidates
        .into_iter()
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, candidate)| candidate)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VendorKind {
    Sell,
    Repair,
    Auction,
}

fn ordered_spawns<'a>(
    spawns: impl IntoIterator<Item = &'a [f32; 5]>,
    map: u32,
    from: Vec3,
) -> Vec<Vec3> {
    let points = spawns.into_iter().filter_map(|spawn| {
        if spawn[0] as u32 != map {
            return None;
        }
        let point = Vec3::new(spawn[1], spawn[2], spawn[3]);
        point.is_finite().then_some(point)
    });
    ordered_points(points, from)
}

fn ordered_points(points: impl IntoIterator<Item = Vec3>, from: Vec3) -> Vec<Vec3> {
    let mut points: Vec<_> = points
        .into_iter()
        .map(|point| (from.distance(point), point))
        .collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut distinct = Vec::new();
    for (_, point) in points {
        if !distinct.iter().any(|other: &Vec3| *other == point) {
            distinct.push(point);
        }
    }
    distinct
}

fn nearest_location(spawn: &KnowledgeSpawn, map: u32) -> Option<Vec3> {
    if spawn.map_id != map {
        return None;
    }
    let point = Vec3::new(spawn.x, spawn.y, spawn.z);
    point.is_finite().then_some(point)
}

pub fn load_azerothcore_catalog(path: impl AsRef<Path>) -> anyhow::Result<AzerothCoreCatalog> {
    let catalog: AzerothCoreCatalog = serde_json::from_slice(&fs::read(path)?)?;
    super::validation::validate_azerothcore_catalog(&catalog).map_err(anyhow::Error::msg)?;
    Ok(catalog)
}

static EMBEDDED: OnceLock<AzerothCoreCatalog> = OnceLock::new();
pub fn embedded_azerothcore_catalog() -> &'static AzerothCoreCatalog {
    EMBEDDED.get_or_init(|| {
        let catalog: AzerothCoreCatalog = serde_json::from_str(super::azerothcore_catalog_json())
            .expect("embedded AzerothCore catalog JSON must parse against the typed schema");
        super::validation::validate_azerothcore_catalog(&catalog)
            .expect("embedded AzerothCore catalog must pass validation");
        catalog
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quest_start(quest_id: u32, allowable_classes: u32, map_id: u32, x: f32) -> QuestStart {
        QuestStart {
            quest_id,
            giver_entry_id: quest_id + 100,
            giver_kind: KnowledgeEntityKind::Creature,
            giver_name: Some(format!("giver-{quest_id}")),
            allowable_classes,
            spawns: vec![KnowledgeSpawn {
                map_id,
                zone_id: 0,
                area_id: 0,
                x,
                y: 0.0,
                z: 0.0,
                orientation: 0.0,
            }],
        }
    }

    #[test]
    fn spawn_candidates_are_nearest_first_and_unique() {
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let spawns = [
            [1.0, 9.0, 0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0, 0.0, 0.0],
            [2.0, 1.0, 0.0, 0.0, 0.0],
        ];
        let candidates = ordered_spawns(spawns.iter(), 1, origin);
        assert_eq!(
            candidates,
            vec![Vec3::new(2.0, 0.0, 0.0), Vec3::new(9.0, 0.0, 0.0)]
        );
    }

    #[test]
    fn quest_starts_filter_by_class_map_and_horizon_then_sort_nearest_first() {
        let starts = [
            quest_start(1, 1 << 2, 1, 8.0), // class 3
            quest_start(2, 1 << 1, 1, 2.0), // class 2
            quest_start(3, 0, 2, 1.0),      // other map
            quest_start(4, 0, 1, 5.0),      // unrestricted
            quest_start(5, 1 << 2, 1, 11.0),
        ];
        let found = nearby_quest_start_locations(&starts, 1, Vec3::default(), Some(3), 10.0);
        assert_eq!(
            found.iter().map(|start| start.quest_id).collect::<Vec<_>>(),
            vec![4, 1]
        );
        assert_eq!(found[0].giver_entry_id, 104);
        assert_eq!(found[0].location, Vec3::new(5.0, 0.0, 0.0));
    }

    #[test]
    fn unknown_class_only_matches_unrestricted_starters_and_bad_horizons_match_none() {
        let starts = [quest_start(1, 1, 1, 1.0), quest_start(2, 0, 1, 2.0)];
        let found = nearby_quest_start_locations(&starts, 1, Vec3::default(), None, 10.0);
        assert_eq!(
            found.iter().map(|start| start.quest_id).collect::<Vec<_>>(),
            vec![2]
        );
        assert!(
            nearby_quest_start_locations(&starts, 1, Vec3::default(), Some(1), f32::NAN).is_empty()
        );
    }
}
