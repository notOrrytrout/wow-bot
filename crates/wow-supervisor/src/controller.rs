use tokio::sync::mpsc;
use wow_domain::{
    ActionId, ControllerContractError, ControllerObservedEntity, ControllerRequest,
    ControllerResponse, ControllerStateView, EntityId, Mission, ProposedAction, TaskId,
    ValidityStamp,
};
use wow_state::Snapshot;

const SCHEDULE_QUEUE_CAPACITY: usize = 8;

/// Bounded handoff for future controller work. This type does not call a model
/// provider; deterministic operation does not create or consume requests.
#[derive(Clone)]
pub struct ControllerScheduler {
    requests: mpsc::Sender<ControllerRequest>,
}

pub struct ControllerRequestReceiver {
    requests: mpsc::Receiver<ControllerRequest>,
}

impl ControllerScheduler {
    pub fn bounded_channel(
        capacity: usize,
    ) -> Result<(Self, ControllerRequestReceiver), ControllerScheduleError> {
        if capacity == 0 || capacity > SCHEDULE_QUEUE_CAPACITY {
            return Err(ControllerScheduleError::InvalidCapacity);
        }
        let (tx, rx) = mpsc::channel(capacity);
        Ok((
            Self { requests: tx },
            ControllerRequestReceiver { requests: rx },
        ))
    }

    pub async fn schedule(
        &self,
        request: ControllerRequest,
    ) -> Result<(), ControllerScheduleError> {
        request
            .validate_bounds()
            .map_err(ControllerScheduleError::InvalidRequest)?;
        self.requests
            .send(request)
            .await
            .map_err(|_| ControllerScheduleError::Closed)
    }
}

impl ControllerRequestReceiver {
    pub async fn recv(&mut self) -> Option<ControllerRequest> {
        self.requests.recv().await
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControllerScheduleError {
    InvalidCapacity,
    InvalidRequest(ControllerContractError),
    Closed,
}

/// Build bounded model context from authoritative state and caller-grounded choices.
pub fn build_request(
    request_id: u64,
    stamp: ValidityStamp,
    mission: &Mission,
    snapshot: &Snapshot,
    choices: Vec<wow_domain::GroundedChoice>,
) -> Result<ControllerRequest, ControllerContractError> {
    if snapshot.revision != stamp.state {
        return Err(ControllerContractError::StaleState);
    }
    let position = snapshot.state.position.player;
    let player = snapshot.state.session.character_guid.map(EntityId);
    let mut nearby_entities: Vec<_> = snapshot
        .state
        .entities
        .0
        .values()
        .filter(|entity| Some(entity.id) != player)
        .filter_map(|entity| {
            let target_position = entity.position?;
            let player_position = position?;
            if target_position.map != player_position.map {
                return None;
            }
            let distance_yards = player_position.point.distance(target_position.point);
            distance_yards
                .is_finite()
                .then_some(ControllerObservedEntity {
                    id: entity.id,
                    entry: entity.entry,
                    distance_yards,
                    hostile: entity.hostile,
                    interactable: entity.interactable,
                })
        })
        .collect();
    nearby_entities.sort_by(|a, b| {
        a.distance_yards
            .total_cmp(&b.distance_yards)
            .then_with(|| a.id.cmp(&b.id))
    });
    nearby_entities.truncate(wow_domain::MAX_CONTROLLER_NEARBY_ENTITIES);
    let mut active_quests: Vec<_> = snapshot.state.quests.active.keys().copied().collect();
    active_quests.truncate(wow_domain::MAX_CONTROLLER_ACTIVE_QUESTS);
    let state = ControllerStateView {
        in_world: snapshot.state.session.in_world,
        position,
        class_id: snapshot.state.capabilities.class_id,
        specialization_tree: snapshot.state.capabilities.specialization_tree,
        active_quests,
        nearby_entities,
    };
    let request = ControllerRequest {
        request_id,
        stamp,
        mission_id: mission.id,
        mission: mission.intent.clone(),
        state,
        choices,
    };
    request.validate_bounds()?;
    Ok(request)
}

/// Reject stale or ungrounded model output, then produce a typed proposal. The
/// lane engine still performs the ordinary ActionValidator checks on this action.
pub fn validate_response(
    request: &ControllerRequest,
    response: &ControllerResponse,
    current: ValidityStamp,
    action_id: ActionId,
    task_id: TaskId,
) -> Result<ProposedAction, ControllerContractError> {
    request.action_for_response(response, current, action_id, task_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_domain::{
        ControllerResponse, EntityId, GroundedChoice, MissionId, MissionIntent, MissionRevision,
        OwnershipGeneration, PermissionRevision, PlanOrigin, StateRevision, WorkerGeneration,
    };
    use wow_state::{
        AuthoritativeState,
        entities::{EntityKind, EntityState},
        quests::QuestProgress,
    };

    fn stamp(state: u64, mission: u64) -> ValidityStamp {
        ValidityStamp {
            state: StateRevision(state),
            mission: MissionRevision(mission),
            permission: PermissionRevision(2),
            worker: WorkerGeneration(3),
            ownership: OwnershipGeneration(4),
            movement: Default::default(),
        }
    }

    fn fixture() -> (Mission, Snapshot, Vec<GroundedChoice>) {
        let mission = Mission::quest(MissionId(7));
        let mut state = AuthoritativeState::default();
        state.revision = StateRevision(5);
        state.session.in_world = true;
        state.session.character_guid = Some(1);
        state.position.player = Some(wow_domain::WorldPosition {
            map: 0,
            point: wow_domain::Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        });
        state.quests.active.insert(
            123,
            QuestProgress {
                complete: false,
                objectives: vec![1],
            },
        );
        state.entities.0.insert(
            EntityId(9),
            EntityState {
                id: EntityId(9),
                entry: 55,
                kind: EntityKind::Unit,
                hostile: true,
                position: state.position.player.map(|mut p| {
                    p.point.x = 4.0;
                    p
                }),
                ..Default::default()
            },
        );
        let choices = vec![GroundedChoice {
            id: 1,
            label: "Attack the observed quest target".into(),
            command: GameplayCommand::Attack(EntityId(9)),
        }];
        (mission, Snapshot::from_state(&state), choices)
    }

    #[tokio::test]
    async fn scheduling_uses_a_bounded_queue_and_does_not_call_a_provider() {
        let (mission, snapshot, choices) = fixture();
        let request = build_request(1, stamp(5, 6), &mission, &snapshot, choices).unwrap();
        let (scheduler, mut receiver) = ControllerScheduler::bounded_channel(1).unwrap();
        scheduler.schedule(request.clone()).await.unwrap();
        assert_eq!(receiver.recv().await, Some(request));
        assert!(ControllerScheduler::bounded_channel(9).is_err());
    }

    #[test]
    fn request_contains_bounded_sanitized_context_and_grounded_choice() {
        let (mission, snapshot, choices) = fixture();
        let request = build_request(1, stamp(5, 6), &mission, &snapshot, choices).unwrap();
        assert_eq!(request.mission, MissionIntent::Quest);
        assert_eq!(request.state.active_quests, vec![123]);
        assert_eq!(request.state.nearby_entities.len(), 1);
        assert_eq!(request.state.nearby_entities[0].id, EntityId(9));
        assert_eq!(request.choices.len(), 1);
        assert!(request.validate_bounds().is_ok());

        let (mission, snapshot, _) = fixture();
        let ungrounded = vec![GroundedChoice {
            id: 1,
            label: "Unobserved".into(),
            command: GameplayCommand::Attack(EntityId(77)),
        }];
        assert_eq!(
            build_request(1, stamp(5, 6), &mission, &snapshot, ungrounded),
            Err(ControllerContractError::UngroundedChoice)
        );
    }

    #[test]
    fn stale_mission_state_and_unoffered_choice_are_rejected() {
        let (mission, snapshot, choices) = fixture();
        let request = build_request(8, stamp(5, 6), &mission, &snapshot, choices).unwrap();
        let response = ControllerResponse {
            request_id: 8,
            stamp: request.stamp,
            choice_id: 1,
        };
        let action =
            validate_response(&request, &response, request.stamp, ActionId(4), TaskId(2)).unwrap();
        assert_eq!(action.origin, PlanOrigin::Llm);
        assert_eq!(action.command, GameplayCommand::Attack(EntityId(9)));

        assert_eq!(
            validate_response(&request, &response, stamp(6, 6), ActionId(4), TaskId(2)),
            Err(ControllerContractError::StaleState)
        );
        assert_eq!(
            validate_response(&request, &response, stamp(5, 7), ActionId(4), TaskId(2)),
            Err(ControllerContractError::StaleMission)
        );
        let wrong_choice = ControllerResponse {
            choice_id: 2,
            ..response
        };
        assert_eq!(
            validate_response(
                &request,
                &wrong_choice,
                request.stamp,
                ActionId(4),
                TaskId(2)
            ),
            Err(ControllerContractError::ChoiceNotOffered)
        );
    }
}
