use crate::{ActionId, EntityId, TaskId, ValidityStamp, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PlanOrigin { Deterministic, SystemPolicy, Llm, Dialogue, Operator, Recovery, GroupPolicy }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GameplayCommand {
    MoveTo(Vec3),
    FaceDirection { orientation: f32 },
    StopMovement,
    ReleaseSpirit,
    QueryCorpse,
    ReclaimCorpse { player: EntityId },
    Attack(EntityId),
    Interact(EntityId),
    UseGameObject(EntityId),
    CastGameObject { spell: u32, target: EntityId, report_use: bool },
    Cast { spell: u32, target: Option<EntityId> },
    MaintainBuff { spell: u32, target: EntityId },
    Loot(EntityId),
    Gather(EntityId),
    Fish,
    UseItem { item: u32, target: Option<EntityId> },
    UseItemInstance { item: u32, item_guid: EntityId, backpack_slot: u8, spell: u32, target: Option<EntityId>, cast_count: u8 },
    QueryQuestGivers,
    QueryQuest { quest: u32 },
    AcceptQuest { quest: u32, giver: EntityId },
    TurnInQuest { quest: u32, giver: EntityId },
    RequestQuestReward { quest: u32, giver: EntityId },
    ChooseQuestReward { quest: u32, giver: EntityId, reward: u32 },
    VendorBuy { vendor: EntityId, item: u32, count: u32 },
    VendorSell { vendor: EntityId, item: u32, count: u32 },
    TradeAccept { generation: u64, gift_only: bool },
    AuctionBuy { query_generation: u64, listing_id: u64, max_buyout: u64 },
    MailTake { mailbox_generation: u64, mail_id: u32 },
    EnterVehicle(EntityId),
    VehicleCast { spell: u32, target: Option<EntityId> },
    Chat { channel: u32, text: String },
    Raw { opcode: u32, body: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposedAction { pub id: ActionId, pub task: TaskId, pub origin: PlanOrigin, pub stamp: ValidityStamp, pub command: GameplayCommand }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendableAction { id: ActionId, task: TaskId, origin: PlanOrigin, stamp: ValidityStamp, command: GameplayCommand }
impl SendableAction {
    pub fn from_validated(value: ValidatedAction) -> Self { Self { id: value.id, task: value.task, origin: value.origin, stamp: value.stamp, command: value.command } }
    pub fn id(&self) -> ActionId { self.id }
    pub fn task(&self) -> TaskId { self.task }
    pub fn origin(&self) -> PlanOrigin { self.origin }
    pub fn stamp(&self) -> ValidityStamp { self.stamp }
    pub fn command(&self) -> &GameplayCommand { &self.command }
    pub fn into_command(self) -> GameplayCommand { self.command }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidatedAction { pub id: ActionId, pub task: TaskId, pub origin: PlanOrigin, pub stamp: ValidityStamp, pub command: GameplayCommand }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MovementRequirement { pub destination: Vec3, pub acceptable_range: f32 }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FacingRequirement { pub orientation: f32, pub tolerance: f32 }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineOfSightRequirement { pub target: EntityId }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ValidationOutcome {
    Sendable(SendableAction),
    NeedsMovement(MovementRequirement),
    NeedsFacing(FacingRequirement),
    NeedsLineOfSight(LineOfSightRequirement),
    Rejected(ActionFailure),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionFailure { pub code: String, pub message: String, pub retryable: bool }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ActivationStage { Observe, Maintain, Move, Act }
