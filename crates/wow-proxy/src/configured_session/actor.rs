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
                    if self.state.ownership.commit(ticket) {
                        if self.update_player_pause(false).await.is_ok() {
                            self.state.player.resumed();
                        }
                        let owner = self.state.ownership.snapshot();
                        tracing::info!(lane=?self.state.lane, generation=?owner.generation, movement_epoch=?owner.movement_epoch, "player idle window elapsed; bot ownership resumed");
                        self.publish_ownership().await;
                    }
                }
                false
            }
            SessionMessage::UpstreamConnected(value) => {
                self.state.upstream_connected = value;
                if !value {
                    self.state.world_authoritative = false;
                }
                self.publish_session_state().await;
                false
            }
            SessionMessage::WorldAuthoritative(value) => {
                self.state.world_authoritative = value;
                self.publish_session_state().await;
                tracing::info!(lane=?self.state.lane, connected=self.state.upstream_connected, in_world=value, "configured world authority state changed");
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
                if owner.permits(ClientKind::Bot)
                    && owner.movement_epoch == epoch
                    && destination.is_finite()
                {
                    let _ = self
                        .upstream_tx
                        .send(GameplayCommand::MoveTo(destination))
                        .await;
                }
                false
            }
            WorkerToProxy::StopMovement { epoch, .. } => {
                let owner = self.state.ownership.snapshot();
                if owner.permits(ClientKind::Bot) && owner.movement_epoch == epoch {
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
    use wow_domain::{AccountId, LaneId, WorkerGeneration};

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
}
