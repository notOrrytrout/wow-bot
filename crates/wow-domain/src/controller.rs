use crate::{
    EntityId, GameplayCommand, MissionId, MissionIntent, ProposedAction, TaskId, ValidityStamp,
    WorldPosition,
};
use serde::{Deserialize, Serialize};

pub const MAX_CONTROLLER_CHOICES: usize = 16;
pub const MAX_CONTROLLER_CHOICE_LABEL_BYTES: usize = 160;
pub const MAX_CONTROLLER_ACTIVE_QUESTS: usize = 16;
pub const MAX_CONTROLLER_NEARBY_ENTITIES: usize = 32;
pub const MAX_CONTROLLER_MISSION_TEXT_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControllerObservedEntity {
    pub id: EntityId,
    pub entry: u32,
    pub distance_yards: f32,
    pub hostile: bool,
    pub interactable: bool,
}

/// A small, privacy-filtered view for strategic planning. It excludes account,
/// character name, chat, packet data, inventory details, and raw state maps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControllerStateView {
    pub in_world: bool,
    pub position: Option<WorldPosition>,
    pub class_id: Option<u8>,
    pub specialization_tree: Option<u8>,
    pub active_quests: Vec<u32>,
    pub nearby_entities: Vec<ControllerObservedEntity>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundedChoice {
    pub id: u16,
    pub label: String,
    pub command: GameplayCommand,
}

/// Immutable request context binds a decision to one mission and state stamp.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControllerRequest {
    pub request_id: u64,
    pub stamp: ValidityStamp,
    pub mission_id: MissionId,
    pub mission: MissionIntent,
    pub state: ControllerStateView,
    pub choices: Vec<GroundedChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerResponse {
    pub request_id: u64,
    pub stamp: ValidityStamp,
    pub choice_id: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControllerContractError {
    EmptyChoices,
    TooManyChoices,
    TooManyActiveQuests,
    TooManyNearbyEntities,
    InvalidStateView,
    DuplicateChoiceId,
    EmptyChoiceLabel,
    ChoiceLabelTooLong,
    MissionTextTooLong,
    UnsafeChoice,
    UngroundedChoice,
    WrongRequest,
    StaleState,
    StaleMission,
    StaleValidityStamp,
    ChoiceNotOffered,
}

impl ControllerRequest {
    pub fn validate_bounds(&self) -> Result<(), ControllerContractError> {
        if self.choices.is_empty() {
            return Err(ControllerContractError::EmptyChoices);
        }
        if self.choices.len() > MAX_CONTROLLER_CHOICES {
            return Err(ControllerContractError::TooManyChoices);
        }
        if self.state.active_quests.len() > MAX_CONTROLLER_ACTIVE_QUESTS {
            return Err(ControllerContractError::TooManyActiveQuests);
        }
        if self.state.nearby_entities.len() > MAX_CONTROLLER_NEARBY_ENTITIES {
            return Err(ControllerContractError::TooManyNearbyEntities);
        }
        if self.state.specialization_tree.is_some_and(|tree| tree >= 3)
            || self.state.position.is_some_and(|position| {
                !position.point.is_finite() || !position.orientation.is_finite()
            })
            || self
                .state
                .nearby_entities
                .iter()
                .any(|entity| !entity.distance_yards.is_finite() || entity.distance_yards < 0.0)
        {
            return Err(ControllerContractError::InvalidStateView);
        }
        match &self.mission {
            MissionIntent::Gather { resource } | MissionIntent::Grind { creature: resource }
                if resource.len() > MAX_CONTROLLER_MISSION_TEXT_BYTES =>
            {
                return Err(ControllerContractError::MissionTextTooLong);
            }
            MissionIntent::Battleground {
                battleground: Some(name),
            } if name.len() > MAX_CONTROLLER_MISSION_TEXT_BYTES => {
                return Err(ControllerContractError::MissionTextTooLong);
            }
            MissionIntent::Goal { text } if text.len() > MAX_CONTROLLER_MISSION_TEXT_BYTES => {
                return Err(ControllerContractError::MissionTextTooLong);
            }
            _ => {}
        }

        let mut ids = std::collections::BTreeSet::new();
        for choice in &self.choices {
            if !ids.insert(choice.id) {
                return Err(ControllerContractError::DuplicateChoiceId);
            }
            if choice.label.trim().is_empty() {
                return Err(ControllerContractError::EmptyChoiceLabel);
            }
            if choice.label.len() > MAX_CONTROLLER_CHOICE_LABEL_BYTES {
                return Err(ControllerContractError::ChoiceLabelTooLong);
            }
            if !crate::PlanOrigin::Llm.permits(&choice.command) {
                return Err(ControllerContractError::UnsafeChoice);
            }
            if !choice_is_grounded(&choice.command, &self.state) {
                return Err(ControllerContractError::UngroundedChoice);
            }
        }
        Ok(())
    }

    pub fn action_for_response(
        &self,
        response: &ControllerResponse,
        current: ValidityStamp,
        action_id: crate::ActionId,
        task_id: TaskId,
    ) -> Result<ProposedAction, ControllerContractError> {
        self.validate_bounds()?;
        if response.request_id != self.request_id {
            return Err(ControllerContractError::WrongRequest);
        }
        if response.stamp.state != self.stamp.state || current.state != self.stamp.state {
            return Err(ControllerContractError::StaleState);
        }
        if response.stamp.mission != self.stamp.mission || current.mission != self.stamp.mission {
            return Err(ControllerContractError::StaleMission);
        }
        if response.stamp != self.stamp || current != self.stamp {
            return Err(ControllerContractError::StaleValidityStamp);
        }
        let command = self
            .choices
            .iter()
            .find(|choice| choice.id == response.choice_id)
            .map(|choice| choice.command.clone())
            .ok_or(ControllerContractError::ChoiceNotOffered)?;
        Ok(ProposedAction {
            id: action_id,
            task: task_id,
            origin: crate::PlanOrigin::Llm,
            stamp: current,
            command,
        })
    }
}

fn choice_is_grounded(command: &GameplayCommand, state: &ControllerStateView) -> bool {
    let target = match command {
        GameplayCommand::Attack(target)
        | GameplayCommand::Interact(target)
        | GameplayCommand::UseGameObject(target)
        | GameplayCommand::Loot(target)
        | GameplayCommand::Gather(target)
        | GameplayCommand::EnterVehicle(target)
        | GameplayCommand::AcceptQuest { giver: target, .. }
        | GameplayCommand::TurnInQuest { giver: target, .. }
        | GameplayCommand::RequestQuestReward { giver: target, .. }
        | GameplayCommand::ChooseQuestReward { giver: target, .. }
        | GameplayCommand::CastGameObject { target, .. }
        | GameplayCommand::VendorBuy { vendor: target, .. }
        | GameplayCommand::VendorSell { vendor: target, .. } => Some(*target),
        GameplayCommand::Cast {
            target: Some(target),
            ..
        }
        | GameplayCommand::MaintainBuff { target, .. }
        | GameplayCommand::VehicleCast {
            target: Some(target),
            ..
        }
        | GameplayCommand::ReclaimCorpse { player: target } => Some(*target),
        GameplayCommand::MoveTo(point) => return point.is_finite(),
        GameplayCommand::FaceDirection { orientation } => return orientation.is_finite(),
        GameplayCommand::Cast { target: None, .. }
        | GameplayCommand::VehicleCast { target: None, .. }
        | GameplayCommand::UseItem { .. }
        | GameplayCommand::UseItemInstance { .. }
        | GameplayCommand::Fish
        | GameplayCommand::StopMovement
        | GameplayCommand::ReleaseSpirit
        | GameplayCommand::QueryCorpse
        | GameplayCommand::TradeAccept { .. }
        | GameplayCommand::AuctionBuy { .. }
        | GameplayCommand::MailTake { .. }
        | GameplayCommand::QueryQuestGivers
        | GameplayCommand::QueryQuest { .. }
        | GameplayCommand::Chat { .. }
        | GameplayCommand::Raw { .. } => return true,
    };
    target.is_some_and(|target| {
        state
            .nearby_entities
            .iter()
            .any(|entity| entity.id == target)
    })
}
