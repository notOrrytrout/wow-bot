pub mod azerothcore;
pub mod generate;
pub mod loader;
pub mod validation;

pub use azerothcore::{
    AzerothCoreCatalog, CatalogProvenance, FishingItem, GatherNode, GeneratorSource,
    HashedSourceFile, KnowledgeEntityKind, QuestActionHintCandidate, QuestCatalogSource,
    QuestItemUseRule, QuestKnowledge, QuestSpellRule, QuestStartLocation, TrainerService,
    VendorKind, VendorService, WorldDataSource, embedded_azerothcore_catalog,
    load_azerothcore_catalog,
};

/// Embedded, versioned AzerothCore static knowledge catalog.
/// Consumers query these facts through their own policy API; the catalog does
/// not grant authority to act on static spawn data.
pub(crate) fn azerothcore_catalog_json() -> &'static str {
    include_str!("../../data/world-knowledge/azerothcore-catalog.json")
}
