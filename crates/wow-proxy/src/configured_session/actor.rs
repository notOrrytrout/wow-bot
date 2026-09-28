use super::state::ConfiguredSessionState;
use crate::{
    ownership::{ClientKind, ControlMode},
    transport_gate::authorize,
};
use std::time::Instant;
use tokio::sync::mpsc;
use wow_control_proto::{
    ActionTransportResult, OwnershipSnapshot as WireOwnership, ProxyToWorker, SupervisorCommand,
    WorkerToProxy,
};
use wow_domain::{GameplayCommand, MovementEpoch, OwnershipGeneration, PauseReasons};
use wow_infra::logging::structured::{DiagnosticLogger, DiagnosticStream};
use wow_state::ProtocolObservation;

pub enum SessionMessage {
    Worker(WorkerToProxy),
    PlayerAttached {
        connection: u64,
    },
    PreparePlayerLogin {
        connection: u64,
        ready: tokio::sync::oneshot::Sender<bool>,
    },
    PlayerDetached {
        connection: u64,
    },
    PlayerMovement {
        at: Instant,
        committed: tokio::sync::oneshot::Sender<()>,
    },
    ChannelActivity {
        until: Instant,
    },
    BotOn {
        committed: Option<tokio::sync::oneshot::Sender<bool>>,
    },
    BotOff {
        committed: Option<tokio::sync::oneshot::Sender<bool>>,
    },
    Tick(Instant),
    UpstreamConnected(bool),
    WorldAuthoritative(bool),
    WorldTransition {
        committed: tokio::sync::oneshot::Sender<(MovementEpoch, u64)>,
    },
    Observation(ProtocolObservation),
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum GameplayDispatch {
    Unfenced(GameplayCommand),
    BotMovement {
        command: GameplayCommand,
        generation: OwnershipGeneration,
        movement_epoch: MovementEpoch,
        sequence: u64,
        world_generation: u64,
    },
}

impl GameplayDispatch {
    pub fn command(&self) -> &GameplayCommand {
        match self {
            Self::Unfenced(command) | Self::BotMovement { command, .. } => command,
        }
    }

    pub fn into_command(self) -> GameplayCommand {
        match self {
            Self::Unfenced(command) | Self::BotMovement { command, .. } => command,
        }
    }

    pub fn bot_movement_stamp(&self) -> Option<(OwnershipGeneration, MovementEpoch, u64, u64)> {
        match self {
            Self::BotMovement {
                generation,
                movement_epoch,
                sequence,
                world_generation,
                ..
            } => Some((*generation, *movement_epoch, *sequence, *world_generation)),
            Self::Unfenced(_) => None,
        }
    }

    pub fn is_current(
        &self,
        owner: crate::ownership::OwnershipSnapshot,
        world_generation: u64,
    ) -> bool {
        match self {
            Self::Unfenced(_) => true,
            Self::BotMovement {
                generation,
                movement_epoch,
                world_generation: stamped_world_generation,
                ..
            } => {
                owner.permits(ClientKind::Bot)
                    && owner.generation == *generation
                    && owner.movement_epoch == *movement_epoch
                    && *stamped_world_generation == world_generation
            }
        }
    }

    pub fn is_new_sequence(&self, last: &mut Option<(u64, u64, u64)>) -> bool {
        let Self::BotMovement {
            generation,
            movement_epoch,
            sequence,
            ..
        } = self
        else {
            return true;
        };
        let owner_stamp = (generation.get(), movement_epoch.get());
        if let Some((last_generation, last_epoch, last_sequence)) = last
            && (*last_generation, *last_epoch) == owner_stamp
        {
            if *sequence <= *last_sequence {
                return false;
            }
            *last_sequence = *sequence;
            return true;
        }
        *last = Some((owner_stamp.0, owner_stamp.1, *sequence));
        true
    }
}

fn bot_movement_rejection_reason(
    owner: crate::ownership::OwnershipSnapshot,
    epoch: MovementEpoch,
    destination_is_finite: bool,
) -> Option<&'static str> {
    if !owner.permits(ClientKind::Bot) {
        Some("bot does not own movement")
    } else if owner.movement_epoch != epoch {
        Some("movement epoch is stale")
    } else if !destination_is_finite {
        Some("destination is not finite")
    } else {
        None
    }
}

pub struct ConfiguredSessionActor {
    pub state: ConfiguredSessionState,
    pub rx: mpsc::Receiver<SessionMessage>,
    pub worker_tx: mpsc::Sender<ProxyToWorker>,
    pub upstream_tx: mpsc::Sender<GameplayDispatch>,
    pub supervisor_tx: mpsc::Sender<SupervisorCommand>,
    pub diagnostics: DiagnosticLogger,
    pub assistance_armed: tokio::sync::watch::Sender<bool>,
    pub ownership_state: tokio::sync::watch::Sender<crate::ownership::OwnershipSnapshot>,
    pub movement_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub world_generation: u64,
    pub movement_sequence: u64,
    pub last_worker_movement_sequence: Option<(wow_domain::WorkerGeneration, u64)>,
}

impl ConfiguredSessionActor {
    async fn lock_movement_gate(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.movement_gate.clone().lock_owned().await
    }

    fn next_movement_sequence(&mut self) -> u64 {
        self.movement_sequence = self.movement_sequence.wrapping_add(1).max(1);
        self.movement_sequence
    }

    pub async fn run(mut self) {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if self.handle(SessionMessage::Tick(Instant::now())).await { break; }
                }
                msg = self.rx.recv() => {
                    let Some(msg) = msg else { break; };
                    if self.handle(msg).await { break; }
                }
            }
        }
    }

    async fn handle(&mut self, msg: SessionMessage) -> bool {
        match msg {
            SessionMessage::Worker(msg) => self.handle_worker(msg).await,
            SessionMessage::PlayerAttached { connection } => {
                self.attach_player(connection).await;
                false
            }
            SessionMessage::PreparePlayerLogin { connection, ready } => {
                let paused = self.attach_player(connection).await;
                let _ = ready.send(paused);
                false
            }
            SessionMessage::PlayerDetached { connection } => {
                let _movement_guard = self.lock_movement_gate().await;
                let final_connection = self.state.player.detach(connection);
                self.diagnostics.record(
                    DiagnosticStream::ProxySession,
                    self.state.lane,
                    "player_detached",
                    serde_json::json!({
                        "final_connection": final_connection,
                    }),
                );
                if final_connection {
                    self.state.upstream_connected = false;
                    self.state.world_authoritative = false;
                    self.world_generation = self.world_generation.wrapping_add(1).max(1);
                    self.publish_session_state().await;
                    if self.state.worker_running {
                        self.state.ownership.player_logout_reclaiming(true);
                        self.publish_ownership().await;
                        let ticket = self.state.ownership.reconnect();
                        if self.state.upstream_connected && self.state.ownership.commit(ticket) {
                            let _ = self.update_player_pause(false).await;
                            self.publish_ownership().await;
                        }
                    } else {
                        self.state.ownership.player_logout_no_bot();
                        self.publish_ownership().await;
                    }
                }
                false
            }
            SessionMessage::PlayerMovement { at, committed } => {
                let movement_guard = self.lock_movement_gate().await;
                self.state.player.moved(at);
                // A player login places the session in explicit manual-off
                // mode. Movement from that client must not turn the bot back
                // into a requested owner or arm an idle resume.
                if self.state.player.explicit_manual_off {
                    let _ = committed.send(());
                    return false;
                }
                // The first real player movement takes control from an active
                // bot. Later movement packets only refresh the idle deadline;
                // they must not publish a new ownership generation each time.
                if !self.state.ownership.snapshot().bot_allowed() {
                    let _ = committed.send(());
                    return false;
                }
                self.state.ownership.player_movement_takeover();
                let owner = self.state.ownership.snapshot();
                tracing::info!(lane=?self.state.lane, generation=?owner.generation, movement_epoch=?owner.movement_epoch, idle_resume_ms=self.state.player.idle_window.as_millis(), "player movement temporarily took locomotion; bot resume armed");
                self.publish_ownership().await;
                let _ = committed.send(());
                drop(movement_guard);
                self.diagnostics.record(
                    DiagnosticStream::ProxyMovement,
                    self.state.lane,
                    "player_movement_takeover",
                    serde_json::json!({
                        "generation": owner.generation.get(),
                        "movement_epoch": owner.movement_epoch.get(),
                        "idle_resume_ms": self.state.player.idle_window.as_millis() as u64,
                    }),
                );
                false
            }
            SessionMessage::ChannelActivity { until } => {
                self.state.player.block_channel_until(until);
                false
            }
            SessionMessage::BotOn { committed: reply } => {
                let _movement_guard = self.lock_movement_gate().await;
                self.state.player.bot_on();
                let ticket = self.state.ownership.begin(ControlMode::Bot);
                self.publish_ownership().await;
                let committed =
                    self.state.upstream_connected && self.state.ownership.commit(ticket);
                if committed {
                    let _ = self.update_player_pause(false).await;
                }
                let owner = self.state.ownership.snapshot();
                tracing::info!(lane=?self.state.lane, committed, generation=?owner.generation, movement_epoch=?owner.movement_epoch, "bot ownership command applied: ON");
                self.publish_ownership().await;
                if let Some(reply) = reply {
                    let _ = reply.send(committed);
                }
                false
            }
            SessionMessage::BotOff { committed: reply } => {
                let _movement_guard = self.lock_movement_gate().await;
                self.state.player.bot_off();
                let ticket = self.state.ownership.begin(ControlMode::Manual);
                self.publish_ownership().await;
                let _ = self.update_player_pause(true).await;
                let committed = self.state.ownership.commit(ticket);
                let owner = self.state.ownership.snapshot();
                tracing::info!(lane=?self.state.lane, committed, generation=?owner.generation, movement_epoch=?owner.movement_epoch, "bot ownership command applied: OFF");
                if committed {
                    self.publish_ownership().await;
                }
                if let Some(reply) = reply {
                    let _ = reply.send(committed);
                }
                false
            }
            SessionMessage::Tick(now) => {
                if self.state.player.should_resume(now) && self.state.upstream_connected {
                    let _movement_guard = self.lock_movement_gate().await;
                    let ticket = self.state.ownership.begin(ControlMode::Bot);
                    match self.update_player_pause(false).await {
                        Ok(()) if self.state.ownership.commit(ticket) => {
                            self.state.player.resumed();
                            let owner = self.state.ownership.snapshot();
                            tracing::info!(lane=?self.state.lane, generation=?owner.generation, movement_epoch=?owner.movement_epoch, "player idle window elapsed; bot ownership resumed");
                            self.diagnostics.record(
                                DiagnosticStream::ProxyMovement,
                                self.state.lane,
                                "bot_movement_resumed",
                                serde_json::json!({
                                    "generation": owner.generation.get(),
                                    "movement_epoch": owner.movement_epoch.get(),
                                }),
                            );
                            self.publish_ownership().await;
                        }
                        Ok(()) => {
                            tracing::warn!(lane=?self.state.lane, "player idle window elapsed; bot resume transition became stale");
                        }
                        Err(error) => {
                            self.state.ownership.fail(ticket);
                            self.state.player.resume_failed(now);
                            tracing::warn!(lane=?self.state.lane, %error, "player idle window elapsed; bot resume request failed");
                            self.publish_ownership().await;
                        }
                    }
                }
                false
            }
            SessionMessage::UpstreamConnected(value) => {
                let _movement_guard = self.lock_movement_gate().await;
                let changed = self.state.upstream_connected != value;
                self.state.upstream_connected = value;
                if !value {
                    self.state.world_authoritative = false;
                }
                if !value && changed {
                    self.state.ownership.fence_movement();
                    self.world_generation = self.world_generation.wrapping_add(1).max(1);
                    self.publish_ownership().await;
                }
                self.publish_session_state().await;
                if changed {
                    self.diagnostics.record(
                        DiagnosticStream::ProxySession,
                        self.state.lane,
                        "upstream_connection_changed",
                        serde_json::json!({
                            "connected": value,
                        }),
                    );
                }
                false
            }
            SessionMessage::WorldAuthoritative(value) => {
                let changed = self.state.world_authoritative != value;
                self.state.world_authoritative = value;
                self.publish_session_state().await;
                tracing::info!(lane=?self.state.lane, connected=self.state.upstream_connected, in_world=value, "configured world authority state changed");
                if changed {
                    self.diagnostics.record(
                        DiagnosticStream::ProxySession,
                        self.state.lane,
                        "world_authority_changed",
                        serde_json::json!({
                            "connected": self.state.upstream_connected,
                            "in_world": value,
                        }),
                    );
                }
                false
            }
            SessionMessage::WorldTransition { committed } => {
                let _movement_guard = self.lock_movement_gate().await;
                self.state.ownership.fence_movement();
                self.world_generation = self.world_generation.wrapping_add(1).max(1);
                let epoch = self.state.ownership.snapshot().movement_epoch;
                self.publish_ownership().await;
                let _ = committed.send((epoch, self.world_generation));
                false
            }
            SessionMessage::Observation(observation) => {
                let _ = self
                    .worker_tx
                    .send(ProxyToWorker::Observation(observation))
                    .await;
                false
            }
            SessionMessage::Shutdown => true,
        }
    }

    async fn attach_player(&mut self, connection: u64) -> bool {
        let _movement_guard = self.lock_movement_gate().await;
        self.state.player.attach(connection);
        let paused = self.update_player_pause(true).await.is_ok();
        self.diagnostics.record(
            DiagnosticStream::ProxySession,
            self.state.lane,
            "player_attached",
            serde_json::json!({"pause_committed": paused}),
        );
        if paused {
            self.state.player.bot_off();
            self.state.ownership.player_attached_manual_off();
            self.publish_ownership().await;
        } else {
            self.state.player.detach(connection);
        }
        paused
    }

    async fn handle_worker(&mut self, msg: WorkerToProxy) -> bool {
        match msg {
            WorkerToProxy::Action(action) => {
                let id = action.id();
                let result =
                    match authorize(&action, self.state.worker, self.state.ownership.snapshot()) {
                        Ok(()) => {
                            let movement_epoch = action.stamp().movement;
                            let command = action.into_command();
                            let dispatch = if matches!(
                                &command,
                                GameplayCommand::MoveTo(_)
                                    | GameplayCommand::FaceDirection { .. }
                                    | GameplayCommand::StopMovement
                            ) {
                                let sequence = self.next_movement_sequence();
                                GameplayDispatch::BotMovement {
                                    command,
                                    generation: self.state.ownership.snapshot().generation,
                                    movement_epoch,
                                    sequence,
                                    world_generation: self.world_generation,
                                }
                            } else {
                                GameplayDispatch::Unfenced(command)
                            };
                            match self.upstream_tx.send(dispatch).await {
                                Ok(()) => ActionTransportResult::Accepted,
                                Err(_) => ActionTransportResult::Rejected {
                                    reason: "upstream closed".into(),
                                },
                            }
                        }
                        Err(error) => ActionTransportResult::Rejected {
                            reason: format!("{error:?}"),
                        },
                    };
                self.diagnostics.record(
                    DiagnosticStream::ProxySession,
                    self.state.lane,
                    "worker_action_transport",
                    serde_json::json!({
                        "action_id": id.get(),
                        "accepted": matches!(result, ActionTransportResult::Accepted),
                    }),
                );
                let _ = self
                    .worker_tx
                    .send(ProxyToWorker::ActionResult { action: id, result })
                    .await;
                false
            }
            WorkerToProxy::QuerySession => {
                self.publish_session_state().await;
                false
            }
            WorkerToProxy::ShutdownAck => true,
            WorkerToProxy::Ready => false,
            WorkerToProxy::Movement {
                movement,
                epoch,
                destination,
            } => {
                let owner = self.state.ownership.snapshot();
                let sequence_is_fresh =
                    self.last_worker_movement_sequence
                        .is_none_or(|(worker, previous)| {
                            worker != self.state.worker || movement.get() > previous
                        });
                let rejection_reason =
                    bot_movement_rejection_reason(owner, epoch, destination.is_finite()).or_else(
                        || {
                            (!sequence_is_fresh)
                                .then_some("movement sequence is stale or duplicated")
                        },
                    );
                let accepted = rejection_reason.is_none();
                self.diagnostics.record(
                    DiagnosticStream::ProxyMovement,
                    self.state.lane,
                    "bot_movement_command",
                    serde_json::json!({
                        "source": "bot",
                        "sequence": movement.get(),
                        "ownership_generation": owner.generation.get(),
                        "movement_epoch": epoch.get(),
                        "world_generation": self.world_generation,
                        "accepted": accepted,
                        "reason": rejection_reason,
                    }),
                );
                if accepted {
                    self.last_worker_movement_sequence = Some((self.state.worker, movement.get()));
                    let sequence = self.next_movement_sequence();
                    let _ = self
                        .upstream_tx
                        .send(GameplayDispatch::BotMovement {
                            command: GameplayCommand::MoveTo(destination),
                            generation: owner.generation,
                            movement_epoch: owner.movement_epoch,
                            sequence,
                            world_generation: self.world_generation,
                        })
                        .await;
                }
                false
            }
            WorkerToProxy::StopMovement { movement, epoch } => {
                let owner = self.state.ownership.snapshot();
                let sequence_is_fresh =
                    self.last_worker_movement_sequence
                        .is_none_or(|(worker, previous)| {
                            worker != self.state.worker || movement.get() > previous
                        });
                let rejection_reason =
                    bot_movement_rejection_reason(owner, epoch, true).or_else(|| {
                        (!sequence_is_fresh).then_some("movement sequence is stale or duplicated")
                    });
                let accepted = rejection_reason.is_none();
                self.diagnostics.record(
                    DiagnosticStream::ProxyMovement,
                    self.state.lane,
                    "bot_stop_movement_command",
                    serde_json::json!({
                        "source": "bot",
                        "sequence": movement.get(),
                        "ownership_generation": owner.generation.get(),
                        "movement_epoch": epoch.get(),
                        "world_generation": self.world_generation,
                        "accepted": accepted,
                        "reason": rejection_reason,
                    }),
                );
                if accepted {
                    self.last_worker_movement_sequence = Some((self.state.worker, movement.get()));
                    let sequence = self.next_movement_sequence();
                    let _ = self
                        .upstream_tx
                        .send(GameplayDispatch::BotMovement {
                            command: GameplayCommand::StopMovement,
                            generation: owner.generation,
                            movement_epoch: owner.movement_epoch,
                            sequence,
                            world_generation: self.world_generation,
                        })
                        .await;
                }
                false
            }
        }
    }

    async fn update_player_pause(&self, paused: bool) -> Result<(), String> {
        self.supervisor_tx
            .send(SupervisorCommand::UpdatePause {
                lane: self.state.lane,
                set: if paused {
                    PauseReasons::PLAYER_CONTROL
                } else {
                    PauseReasons::empty()
                },
                clear: if paused {
                    PauseReasons::empty()
                } else {
                    PauseReasons::PLAYER_CONTROL
                },
            })
            .await
            .map_err(|_| "supervisor unavailable".into())
    }

    async fn publish_session_state(&self) {
        self.diagnostics.record(
            DiagnosticStream::ProxySession,
            self.state.lane,
            "session_state_published",
            serde_json::json!({
                "connected": self.state.upstream_connected,
                "in_world": self.state.world_authoritative,
            }),
        );
        let _ = self
            .worker_tx
            .send(ProxyToWorker::SessionState {
                connected: self.state.upstream_connected,
                in_world: self.state.world_authoritative,
            })
            .await;
    }

    async fn publish_ownership(&self) {
        let owner = self.state.ownership.snapshot();
        self.ownership_state.send_replace(owner);
        self.assistance_armed
            .send_replace(owner.bot_assistance_armed());
        self.diagnostics.record(
            DiagnosticStream::ProxyMovement,
            self.state.lane,
            "ownership_published",
            serde_json::json!({
                "generation": owner.generation.get(),
                "movement_epoch": owner.movement_epoch.get(),
                "bot_allowed": owner.bot_allowed(),
                "player_present": owner.player_present(),
            }),
        );
        let _ = self
            .worker_tx
            .send(ProxyToWorker::OwnershipChanged(WireOwnership {
                generation: owner.generation,
                movement_epoch: owner.movement_epoch,
                player_present: owner.player_present(),
                bot_allowed: owner.bot_allowed(),
            }))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wow_domain::{AccountId, LaneId, MovementId, Vec3, WorkerGeneration};

    fn test_diagnostics() -> DiagnosticLogger {
        DiagnosticLogger::new("test", Vec::new())
    }

    #[tokio::test]
    async fn older_player_disconnect_preserves_newer_world_session() {
        let (_, rx) = mpsc::channel(8);
        let (worker_tx, _worker_rx) = mpsc::channel(32);
        let (upstream_tx, _upstream_rx) = mpsc::channel(8);
        let (supervisor_tx, _supervisor_rx) = mpsc::channel(8);
        let mut actor = ConfiguredSessionActor {
            state: ConfiguredSessionState::new(AccountId(1), LaneId(1), WorkerGeneration::ZERO),
            rx,
            worker_tx,
            upstream_tx,
            supervisor_tx,
            diagnostics: test_diagnostics(),
            assistance_armed: tokio::sync::watch::channel(false).0,
            ownership_state: tokio::sync::watch::channel(
                crate::ownership::AccountOwnership::default().snapshot(),
            )
            .0,
            movement_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            world_generation: 1,
            movement_sequence: 0,
            last_worker_movement_sequence: None,
        };
        actor
            .handle(SessionMessage::PlayerAttached { connection: 1 })
            .await;
        actor
            .handle(SessionMessage::PlayerAttached { connection: 2 })
            .await;
        actor.handle(SessionMessage::UpstreamConnected(true)).await;
        actor.handle(SessionMessage::WorldAuthoritative(true)).await;

        actor
            .handle(SessionMessage::PlayerDetached { connection: 1 })
            .await;
        assert_eq!(actor.state.player.count(), 1);
        assert!(actor.state.upstream_connected);
        assert!(actor.state.world_authoritative);

        let (bot_on_tx, bot_on_rx) = tokio::sync::oneshot::channel();
        actor
            .handle(SessionMessage::BotOn {
                committed: Some(bot_on_tx),
            })
            .await;
        assert_eq!(bot_on_rx.await, Ok(true));
        assert!(actor.state.ownership.snapshot().bot_allowed());
        let (bot_off_tx, bot_off_rx) = tokio::sync::oneshot::channel();
        actor
            .handle(SessionMessage::BotOff {
                committed: Some(bot_off_tx),
            })
            .await;
        assert_eq!(bot_off_rx.await, Ok(true));
        assert!(!actor.state.ownership.snapshot().bot_allowed());

        actor
            .handle(SessionMessage::PlayerDetached { connection: 2 })
            .await;
        assert_eq!(actor.state.player.count(), 0);
        assert!(!actor.state.upstream_connected);
        assert!(!actor.state.world_authoritative);
    }

    #[tokio::test]
    async fn idle_resume_is_published_only_after_supervisor_accepts_resume() {
        let (_, rx) = mpsc::channel(8);
        let (worker_tx, mut worker_rx) = mpsc::channel(8);
        let (upstream_tx, _upstream_rx) = mpsc::channel(8);
        let (supervisor_tx, mut supervisor_rx) = mpsc::channel(8);
        let (assistance_armed, assistance_rx) = tokio::sync::watch::channel(false);
        let mut actor = ConfiguredSessionActor {
            state: ConfiguredSessionState::new(AccountId(1), LaneId(1), WorkerGeneration::ZERO),
            rx,
            worker_tx,
            upstream_tx,
            supervisor_tx,
            diagnostics: test_diagnostics(),
            assistance_armed,
            ownership_state: tokio::sync::watch::channel(
                crate::ownership::AccountOwnership::default().snapshot(),
            )
            .0,
            movement_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            world_generation: 1,
            movement_sequence: 0,
            last_worker_movement_sequence: None,
        };
        let moved_at = Instant::now();
        actor.state.upstream_connected = true;
        actor.state.player.bot_on();
        actor.state.player.moved(moved_at);
        actor.state.ownership.player_attached_bot_on();
        actor.state.ownership.player_movement_takeover();
        actor.publish_ownership().await;
        assert!(*assistance_rx.borrow());
        assert!(matches!(
            worker_rx.recv().await,
            Some(ProxyToWorker::OwnershipChanged(owner)) if !owner.bot_allowed
        ));

        actor
            .handle(SessionMessage::Tick(
                moved_at + actor.state.player.idle_window,
            ))
            .await;

        let command = supervisor_rx.recv().await.expect("resume request");
        assert!(matches!(
            command,
            SupervisorCommand::UpdatePause { lane: LaneId(1), set, clear }
                if set.is_empty() && clear.contains(PauseReasons::PLAYER_CONTROL)
        ));
        assert!(!actor.state.player.idle_resume_armed);
        assert!(actor.state.ownership.snapshot().bot_allowed());
        assert!(matches!(
            worker_rx.recv().await,
            Some(ProxyToWorker::OwnershipChanged(owner)) if owner.bot_allowed
        ));

        let ticket = actor.state.ownership.begin(ControlMode::Manual);
        assert!(actor.state.ownership.commit(ticket));
        actor.publish_ownership().await;
        assert!(!*assistance_rx.borrow());
    }

    #[tokio::test]
    async fn failed_idle_resume_keeps_resume_armed_and_does_not_publish_bot_ownership() {
        let (_, rx) = mpsc::channel(8);
        let (worker_tx, mut worker_rx) = mpsc::channel(8);
        let (upstream_tx, _upstream_rx) = mpsc::channel(8);
        let (supervisor_tx, supervisor_rx) = mpsc::channel(8);
        drop(supervisor_rx);
        let mut actor = ConfiguredSessionActor {
            state: ConfiguredSessionState::new(AccountId(1), LaneId(1), WorkerGeneration::ZERO),
            rx,
            worker_tx,
            upstream_tx,
            supervisor_tx,
            diagnostics: test_diagnostics(),
            assistance_armed: tokio::sync::watch::channel(false).0,
            ownership_state: tokio::sync::watch::channel(
                crate::ownership::AccountOwnership::default().snapshot(),
            )
            .0,
            movement_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            world_generation: 1,
            movement_sequence: 0,
            last_worker_movement_sequence: None,
        };
        let moved_at = Instant::now();
        actor.state.upstream_connected = true;
        actor.state.player.bot_on();
        actor.state.player.moved(moved_at);
        actor.state.ownership.player_attached_bot_on();
        actor.state.ownership.player_movement_takeover();

        let idle_deadline = moved_at + actor.state.player.idle_window;
        actor.handle(SessionMessage::Tick(idle_deadline)).await;

        assert!(actor.state.player.idle_resume_armed);
        assert!(!actor.state.ownership.snapshot().bot_allowed());
        assert!(matches!(
            worker_rx.recv().await,
            Some(ProxyToWorker::OwnershipChanged(owner)) if !owner.bot_allowed
        ));
        actor
            .handle(SessionMessage::Tick(
                idle_deadline + Duration::from_millis(250),
            ))
            .await;
        assert!(worker_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn queued_bot_movement_is_rejected_after_takeover_or_world_transition() {
        let (_, rx) = mpsc::channel(8);
        let (worker_tx, _worker_rx) = mpsc::channel(8);
        let (upstream_tx, mut upstream_rx) = mpsc::channel(8);
        let (supervisor_tx, _supervisor_rx) = mpsc::channel(8);
        let mut actor = ConfiguredSessionActor {
            state: ConfiguredSessionState::new(AccountId(1), LaneId(1), WorkerGeneration::ZERO),
            rx,
            worker_tx,
            upstream_tx,
            supervisor_tx,
            diagnostics: test_diagnostics(),
            assistance_armed: tokio::sync::watch::channel(false).0,
            ownership_state: tokio::sync::watch::channel(
                crate::ownership::AccountOwnership::default().snapshot(),
            )
            .0,
            movement_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            world_generation: 1,
            movement_sequence: 0,
            last_worker_movement_sequence: None,
        };
        actor.state.ownership.player_attached_bot_on();
        let first_epoch = actor.state.ownership.snapshot().movement_epoch;
        actor
            .handle(SessionMessage::Worker(WorkerToProxy::Movement {
                movement: MovementId(1),
                epoch: first_epoch,
                destination: Vec3::new(1.0, 2.0, 3.0),
            }))
            .await;
        let queued = upstream_rx.recv().await.expect("bot movement should queue");
        assert!(queued.is_current(actor.state.ownership.snapshot(), actor.world_generation));
        let mut last_sequence = None;
        assert!(queued.is_new_sequence(&mut last_sequence));
        assert!(!queued.is_new_sequence(&mut last_sequence));
        actor
            .handle(SessionMessage::Worker(WorkerToProxy::Movement {
                movement: MovementId(1),
                epoch: first_epoch,
                destination: Vec3::new(1.5, 2.0, 3.0),
            }))
            .await;
        assert!(upstream_rx.try_recv().is_err());

        actor.state.ownership.player_movement_takeover();
        assert!(!queued.is_current(actor.state.ownership.snapshot(), actor.world_generation));

        let ticket = actor.state.ownership.begin(ControlMode::Bot);
        assert!(actor.state.ownership.commit(ticket));
        let current_epoch = actor.state.ownership.snapshot().movement_epoch;
        actor
            .handle(SessionMessage::Worker(WorkerToProxy::Movement {
                movement: MovementId(2),
                epoch: current_epoch,
                destination: Vec3::new(4.0, 5.0, 6.0),
            }))
            .await;
        let queued_before_world_change = upstream_rx
            .recv()
            .await
            .expect("second bot movement should queue");
        let (committed, receiver) = tokio::sync::oneshot::channel();
        actor
            .handle(SessionMessage::WorldTransition { committed })
            .await;
        let (fenced_epoch, fenced_world_generation) =
            receiver.await.expect("world movement fence should commit");
        assert_ne!(fenced_epoch, current_epoch);
        assert!(
            !queued_before_world_change
                .is_current(actor.state.ownership.snapshot(), fenced_world_generation)
        );
        let current_owner = actor.state.ownership.snapshot();
        let stale_world_dispatch = GameplayDispatch::BotMovement {
            command: GameplayCommand::StopMovement,
            generation: current_owner.generation,
            movement_epoch: current_owner.movement_epoch,
            sequence: 3,
            world_generation: 1,
        };
        assert!(!stale_world_dispatch.is_current(current_owner, fenced_world_generation));
    }

    #[tokio::test]
    async fn ownership_handoff_waits_for_inflight_movement_write_gate() {
        let (_, rx) = mpsc::channel(8);
        let (worker_tx, _worker_rx) = mpsc::channel(8);
        let (upstream_tx, _upstream_rx) = mpsc::channel(8);
        let (supervisor_tx, _supervisor_rx) = mpsc::channel(8);
        let mut state =
            ConfiguredSessionState::new(AccountId(1), LaneId(1), WorkerGeneration::ZERO);
        state.ownership.player_attached_bot_on();
        let movement_gate = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let mut actor = ConfiguredSessionActor {
            state,
            rx,
            worker_tx,
            upstream_tx,
            supervisor_tx,
            diagnostics: test_diagnostics(),
            assistance_armed: tokio::sync::watch::channel(true).0,
            ownership_state: tokio::sync::watch::channel(
                crate::ownership::AccountOwnership::default().snapshot(),
            )
            .0,
            movement_gate: movement_gate.clone(),
            world_generation: 1,
            movement_sequence: 0,
            last_worker_movement_sequence: None,
        };
        let held_write = movement_gate.lock_owned().await;
        let (committed, mut receiver) = tokio::sync::oneshot::channel();
        let handoff = tokio::spawn(async move {
            actor
                .handle(SessionMessage::PlayerMovement {
                    at: Instant::now(),
                    committed,
                })
                .await;
            actor.state.ownership.snapshot()
        });

        tokio::task::yield_now().await;
        assert!(receiver.try_recv().is_err());
        drop(held_write);

        tokio::time::timeout(Duration::from_secs(1), receiver)
            .await
            .expect("handoff should finish after the in-flight write")
            .expect("handoff commit should be sent");
        let owner = handoff.await.expect("session actor task should finish");
        assert!(!owner.bot_allowed());
    }
}
