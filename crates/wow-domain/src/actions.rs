use crate::{ActionId, EntityId, TaskId, ValidityStamp, Vec3};
use serde::{Deserialize, Serialize};

/// Spell used by the captured WotLK client to open quest-item game objects.
pub const QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID: u32 = 6_478;
/// Keep this amount of copper after an automatic maintenance purchase.
pub const MAINTENANCE_PURCHASE_MONEY_RESERVE_COPPER: u64 = 1_000;
/// Limit each automatic vendor purchase to one server-defined lot.
pub const MAX_MAINTENANCE_VENDOR_BUY_LOTS: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlanOrigin {
    Deterministic,
    SystemPolicy,
    Llm,
    Dialogue,
    Operator,
    Recovery,
    GroupPolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GameplayCommand {
    MoveTo(Vec3),
    FaceDirection {
        orientation: f32,
    },
    StopMovement,
    ReleaseSpirit,
    QueryCorpse,
    ReclaimCorpse {
        player: EntityId,
    },
    Attack(EntityId),
    Interact(EntityId),
    UseGameObject(EntityId),
    CastGameObject {
        spell: u32,
        target: EntityId,
        report_use: bool,
    },
    Cast {
        spell: u32,
        target: Option<EntityId>,
    },
    CastOnItem {
        spell: u32,
        item_guid: EntityId,
    },
    MaintainBuff {
        spell: u32,
        target: EntityId,
    },
    SummonPet {
        spell: u32,
        player: EntityId,
    },
    EquipItem {
        item_guid: EntityId,
        destination_slot: u8,
    },
    PetSetReaction {
        pet: EntityId,
        reaction: u8,
    },
    PetAttack {
        pet: EntityId,
        target: EntityId,
    },
    PetSetAutocast {
        pet: EntityId,
        spell: u32,
        enabled: bool,
    },
    Loot(EntityId),
    Gather(EntityId),
    Fish,
    UseItem {
        item: u32,
        target: Option<EntityId>,
    },
    UseItemInstance {
        item: u32,
        item_guid: EntityId,
        backpack_slot: u8,
        spell: u32,
        target: Option<EntityId>,
        cast_count: u8,
    },
    UseItemOnItem {
        item: u32,
        item_guid: EntityId,
        backpack_slot: u8,
        spell: u32,
        target_item_guid: EntityId,
    },
    QueryQuestGivers,
    QueryQuest {
        quest: u32,
    },
    QueryItem {
        item: u32,
    },
    AcceptQuest {
        quest: u32,
        giver: EntityId,
    },
    TurnInQuest {
        quest: u32,
        giver: EntityId,
    },
    RequestQuestReward {
        quest: u32,
        giver: EntityId,
    },
    ChooseQuestReward {
        quest: u32,
        giver: EntityId,
        reward: u32,
    },
    VendorBuy {
        vendor: EntityId,
        item: u32,
        slot: u32,
        count: u32,
    },
    VendorList {
        vendor: EntityId,
    },
    SetAmmo {
        item: u32,
    },
    TrainerList {
        trainer: EntityId,
    },
    TrainerBuy {
        trainer: EntityId,
        spell: u32,
    },
    VendorSell {
        vendor: EntityId,
        item: u32,
        item_guid: EntityId,
        count: u32,
    },
    RepairEquipment {
        vendor: EntityId,
    },
    BankActivate {
        banker: EntityId,
    },
    BankDeposit {
        banker: EntityId,
        item: u32,
        item_guid: EntityId,
        backpack_slot: u8,
    },
    TradeAccept {
        generation: u64,
        gift_only: bool,
    },
    AuctionBuy {
        query_generation: u64,
        listing_id: u64,
        max_buyout: u64,
    },
    MailboxList {
        mailbox: EntityId,
    },
    MailTake {
        mailbox_generation: u64,
        mailbox: EntityId,
        mail_id: u32,
        target: MailTakeTarget,
    },
    EnterVehicle(EntityId),
    VehicleCast {
        spell: u32,
        target: Option<EntityId>,
    },
    Chat {
        channel: u32,
        text: String,
    },
    Raw {
        opcode: u32,
        body: Vec<u8>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MailTakeTarget {
    Money,
    Attachment { low_guid: u32 },
}

impl PlanOrigin {
    /// Check command classes that are forbidden to an action origin.
    pub fn permits(self, command: &GameplayCommand) -> bool {
        match self {
            Self::Dialogue => matches!(command, GameplayCommand::Chat { .. }),
            Self::Llm => !matches!(
                command,
                GameplayCommand::Raw { .. }
                    | GameplayCommand::Chat { .. }
                    | GameplayCommand::TradeAccept { .. }
                    | GameplayCommand::AuctionBuy { .. }
                    | GameplayCommand::MailTake { .. }
                    | GameplayCommand::BankDeposit { .. }
            ),
            Self::Recovery => !matches!(
                command,
                GameplayCommand::Raw { .. } | GameplayCommand::Chat { .. }
            ),
            Self::Deterministic | Self::SystemPolicy | Self::GroupPolicy => !matches!(
                command,
                GameplayCommand::Raw { .. }
                    | GameplayCommand::Chat { .. }
                    | GameplayCommand::ReleaseSpirit
                    | GameplayCommand::QueryCorpse
                    | GameplayCommand::ReclaimCorpse { .. }
            ),
            Self::Operator => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposedAction {
    pub id: ActionId,
    pub task: TaskId,
    pub origin: PlanOrigin,
    pub stamp: ValidityStamp,
    pub command: GameplayCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendableAction {
    id: ActionId,
    task: TaskId,
    origin: PlanOrigin,
    stamp: ValidityStamp,
    command: GameplayCommand,
}
impl SendableAction {
    pub fn from_validated(value: ValidatedAction) -> Self {
        Self {
            id: value.id,
            task: value.task,
            origin: value.origin,
            stamp: value.stamp,
            command: value.command,
        }
    }
    pub fn id(&self) -> ActionId {
        self.id
    }
    pub fn task(&self) -> TaskId {
        self.task
    }
    pub fn origin(&self) -> PlanOrigin {
        self.origin
    }
    pub fn stamp(&self) -> ValidityStamp {
        self.stamp
    }
    pub fn command(&self) -> &GameplayCommand {
        &self.command
    }
    pub fn into_command(self) -> GameplayCommand {
        self.command
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidatedAction {
    pub id: ActionId,
    pub task: TaskId,
    pub origin: PlanOrigin,
    pub stamp: ValidityStamp,
    pub command: GameplayCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MovementRequirement {
    pub destination: Vec3,
    pub acceptable_range: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacingRequirement {
    pub orientation: f32,
    pub tolerance: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineOfSightRequirement {
    pub target: EntityId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ValidationOutcome {
    Sendable(SendableAction),
    NeedsMovement(MovementRequirement),
    NeedsFacing(FacingRequirement),
    NeedsLineOfSight(LineOfSightRequirement),
    Rejected(ActionFailure),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ActivationStage {
    Observe,
    Maintain,
    Move,
    Act,
}
