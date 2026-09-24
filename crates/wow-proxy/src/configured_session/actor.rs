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
use wow_domain::{GameplayCommand, PauseReasons};
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
    },
    ChannelActivity {
        until: Instant,
    },
    BotOn,
    BotOff,
    Tick(Instant),
    UpstreamConnected(bool),
    WorldAuthoritative(bool),
    Observation(ProtocolObservation),
    Shutdown,
}

pub struct ConfiguredSessionActor {
    pub state: ConfiguredSessionState,
    pub rx: mpsc::Receiver<SessionMessage>,
    pub worker_tx: mpsc::Sender<ProxyToWorker>,
    pub upstream_tx: mpsc::Sender<GameplayCommand>,
    pub supervisor_tx: mpsc::Sender<SupervisorCommand>,
    pub diagnostics: DiagnosticLogger,
}

impl ConfiguredSessionActor {
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
            SessionMessage::PlayerMovement { at } => {
                self.state.player.moved(at);
                self.state.ownership.player_movement_takeover();
                let owner = self.state.ownership.snapshot();
                tracing::info!(lane=?self.state.lane, generation=?owner.generation, movement_epoch=?owner.movement_epoch, idle_resume_ms=self.state.player.idle_window.as_millis(), "player movement temporarily took locomotion; bot resume armed");
                self.publish_ownership().await;
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
            SessionMessage::BotOn => {
                self.state.player.bot_on();
                let ticket = self.state.ownership.begin(ControlMode::Bot);
                let committed =
                    self.state.upstream_connected && self.state.ownership.commit(ticket);
                if committed {
                    let _ = self.update_player_pause(false).await;
                }
                let owner = self.state.ownership.snapshot();
                tracing::info!(lane=?self.state.lane, committed, generation=?owner.generation, movement_epoch=?owner.movement_epoch, "bot ownership command applied: ON");
                self.publish_ownership().await;
                false
            }
            SessionMessage::BotOff => {
                self.state.player.bot_off();
                let ticket = self.state.ownership.begin(ControlMode::Manual);
                let _ = self.update_player_pause(true).await;
                let committed = self.state.ownership.commit(ticket);
                let owner = self.state.ownership.snapshot();
                tracing::info!(lane=?self.state.lane, committed, generation=?owner.generation, movement_epoch=?owner.movement_epoch, "bot ownership command applied: OFF");
                if committed {
                    self.publish_ownership().await;
                }
                false
            }
            SessionMessage::Tick(now) => {
                if self.state.player.should_resume(now) && self.state.upstream_connected {
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
                let changed = self.state.upstream_connected != value;
                self.state.upstream_connected = value;
                if !value {
                    self.state.world_authoritative = false;
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
                        Ok(()) => match self.upstream_tx.send(action.into_command()).await {
                            Ok(()) => ActionTransportResult::Accepted,
                            Err(_) => ActionTransportResult::Rejected {
                                reason: "upstream closed".into(),
                            },
                        },
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
                epoch, destination, ..
            } => {
                let owner = self.state.ownership.snapshot();
                let accepted = owner.permits(ClientKind::Bot)
                    && owner.movement_epoch == epoch
                    && destination.is_finite();
                self.diagnostics.record(
                    DiagnosticStream::ProxyMovement,
                    self.state.lane,
                    "bot_movement_command",
                    serde_json::json!({
                        "movement_epoch": epoch.get(),
                        "accepted": accepted,
                    }),
                );
                if accepted {
                    let _ = self
                        .upstream_tx
                        .send(GameplayCommand::MoveTo(destination))
                        .await;
                }
                false
            }
            WorkerToProxy::StopMovement { epoch, .. } => {
                let owner = self.state.ownership.snapshot();
                let accepted = owner.permits(ClientKind::Bot) && owner.movement_epoch == epoch;
                self.diagnostics.record(
                    DiagnosticStream::ProxyMovement,
                    self.state.lane,
                    "bot_stop_movement_command",
                    serde_json::json!({
                        "movement_epoch": epoch.get(),
                        "accepted": accepted,
                    }),
                );
                if accepted {
                    let _ = self.upstream_tx.send(GameplayCommand::StopMovement).await;
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
    use wow_domain::{AccountId, LaneId, WorkerGeneration};

    fn test_diagnostics() -> DiagnosticLogger {
        DiagnosticLogger::new("test", Vec::new())
    }

    #[tokio::test]
    async fn older_player_disconnect_preserves_newer_world_session() {
        let (_, rx) = mpsc::channel(8);
        let (worker_tx, _worker_rx) = mpsc::channel(8);
        let (upstream_tx, _upstream_rx) = mpsc::channel(8);
        let (supervisor_tx, _supervisor_rx) = mpsc::channel(8);
        let mut actor = ConfiguredSessionActor {
            state: ConfiguredSessionState::new(AccountId(1), LaneId(1), WorkerGeneration::ZERO),
            rx,
            worker_tx,
            upstream_tx,
            supervisor_tx,
            diagnostics: test_diagnostics(),
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

        actor.handle(SessionMessage::BotOn).await;
        assert!(actor.state.ownership.snapshot().bot_allowed());

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
        let mut actor = ConfiguredSessionActor {
            state: ConfiguredSessionState::new(AccountId(1), LaneId(1), WorkerGeneration::ZERO),
            rx,
            worker_tx,
            upstream_tx,
            supervisor_tx,
            diagnostics: test_diagnostics(),
        };
        let moved_at = Instant::now();
        actor.state.upstream_connected = true;
        actor.state.player.bot_on();
        actor.state.player.moved(moved_at);
        actor.state.ownership.player_attached_bot_on();
        actor.state.ownership.player_movement_takeover();

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
}
