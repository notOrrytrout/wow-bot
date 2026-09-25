use serde::{Deserialize, Serialize};
use wow_domain::{EntityId, WorldPosition};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LootOwnership {
    Bot,
    Player,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProtocolObservation {
    Authenticated {
        realm: Option<String>,
    },
    EnteredWorld {
        character_guid: u64,
        position: Option<WorldPosition>,
    },
    LeftWorld,
    PlayerPosition {
        position: WorldPosition,
        moving: bool,
        flags: u32,
        client_time: u32,
    },
    ControlledMover {
        mover: Option<EntityId>,
        position: Option<WorldPosition>,
        flags: u32,
    },
    Transport {
        state: crate::transport::TransportState,
    },
    ControlledAbilities {
        mover: EntityId,
        spells: Vec<u32>,
    },
    PetControl {
        pet: Option<EntityId>,
    },
    CastFailed {
        spell: u32,
        reason: u8,
        target: Option<EntityId>,
    },
    CastStarted {
        caster: EntityId,
        spell: u32,
        started_at_ms: u64,
        ends_at_ms: u64,
    },
    CastFinished {
        caster: EntityId,
        spell: u32,
    },
    CastUpdated {
        caster: EntityId,
        ends_at_ms: u64,
    },
    CorpseLocation {
        position: Option<WorldPosition>,
    },
    CorpseReclaimDelay {
        ready_at_ms: u64,
    },
    EntityUpsert {
        entity: crate::entities::EntityState,
    },
    EntityRemoved {
        entity: EntityId,
    },
    InventoryCount {
        item: u32,
        count: u32,
    },
    InventoryInstances {
        items: Vec<crate::inventory::InventoryItemInstance>,
    },
    InventoryFreeSlots {
        count: u16,
    },
    EquippedRangedItem {
        item: Option<u32>,
    },
    EquippedItems {
        items: Option<std::collections::BTreeMap<u8, u32>>,
    },
    Money {
        copper: u64,
    },
    LootOpened {
        target: EntityId,
        ownership: LootOwnership,
    },
    LootClosed {
        ownership: LootOwnership,
    },
    VendorOpened {
        vendor: EntityId,
    },
    VendorClosed,
    Trade(crate::inventory::TradeState),
    Auction(crate::inventory::AuctionState),
    Mailbox(crate::inventory::MailboxState),
    QuestGiverStatus {
        giver: EntityId,
        status: u8,
    },
    QuestGiverListReceived {
        giver: EntityId,
        offer_count: u8,
    },
    QuestOffer {
        giver: EntityId,
        quest: u32,
        icon: u32,
    },
    QuestAccepted {
        quest: u32,
    },
    QuestDefinition {
        definition: crate::quests::QuestDefinition,
    },
    QuestTurnInDialog {
        quest: u32,
        dialog: crate::quests::QuestTurnInDialog,
    },
    QuestProgress {
        quest: u32,
        objectives: Vec<u32>,
        complete: bool,
    },
    QuestRemoved {
        quest: u32,
    },
    QuestCompleted {
        quest: u32,
    },
    SpellKnown {
        spell: u32,
    },
    SpellCooldown {
        spell: u32,
        ready_at_ms: u64,
    },
    SpellGlobalCooldown {
        spell: u32,
        started_at_ms: u64,
    },
    PlayerRunes {
        runes: Option<Vec<crate::capabilities::RuneState>>,
    },
    ComboPoints {
        target: Option<EntityId>,
        points: Option<u8>,
    },
    PlayerClass {
        class_id: u8,
    },
    PlayerTalents {
        group_count: Option<u8>,
        active_group: Option<u8>,
        talents: Vec<crate::capabilities::TalentRank>,
    },
    AuraSnapshot {
        entity: EntityId,
        auras: Vec<crate::auras::AuraInstance>,
    },
    AuraSlot {
        entity: EntityId,
        slot: u8,
        aura: Option<crate::auras::AuraInstance>,
    },
    Skill {
        skill: u32,
        current: u16,
        max: u16,
    },
    ProfessionSnapshot {
        skills: std::collections::BTreeMap<u32, (u16, u16)>,
        slots: std::collections::BTreeMap<usize, u32>,
    },
    RecipeKnown {
        recipe: u32,
    },
    ProfessionFlags {
        cooking: bool,
        first_aid: bool,
        fishing: bool,
    },
    Group(crate::group::GroupState),
    Desync {
        reason: String,
    },
    Resynchronized,
    Raw {
        opcode: u32,
        body: Vec<u8>,
    },
}
