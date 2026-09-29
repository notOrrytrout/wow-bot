mod gameplay;
mod packet;
mod protocol_observations;

use crate::{
    auth::{
        limits::{AdmissionController, AdmissionLimits},
        login::{ConfiguredCredential, UpstreamAuthConfig, upstream_login},
    },
    configured_session::{ConfiguredSessionActor, SessionMessage, state::ConfiguredSessionState},
    framing::{
        ClientFrame,
        client_edge::{read_client_frame, write_client_frame},
        upstream_edge::{read_server_frame, write_server_frame},
    },
    transparent_session::relay::relay as transparent_relay,
    warden::{client::WardenClient, relay::WardenBridge},
};
use anyhow::{Context, Result, bail};
use gameplay::*;
use protocol_observations::*;
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::OpenOptions,
    io::Write as _,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{RwLock, broadcast, mpsc, oneshot, watch},
};
use wow_control_proto::{ProxyToWorker, SupervisorCommand, WorkerToProxy};
#[cfg(test)]
use wow_domain::Vec3;
use wow_domain::{
    AccountId, EntityId, GameplayCommand, LaneId, Mission, MissionId,
    QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID, WorkerGeneration, WorldPosition,
};
use wow_infra::logging::structured::{DiagnosticLogger, DiagnosticStream, diagnostic_path};
use wow_login_messages::Message as _;
use wow_login_messages::{
    all::ProtocolVersion,
    helper::{
        tokio_expect_client_message, tokio_expect_client_message_protocol,
        tokio_expect_server_message_protocol,
    },
    version_8::{CMD_REALM_LIST_Client, CMD_REALM_LIST_Server, Realm, RealmCategory, RealmType},
};
use wow_srp::{normalized_string::NormalizedString, wrath_header::ProofSeed};
use wow_state::ProtocolObservation;
use wow_tentacli_adapter::runtime::{ObjectObservationRuntime, login_verify_world, new_world};
use wow_world_messages::Message as _;
use wow_world_messages::wrath::{
    CMSG_AUTH_SESSION, CMSG_WARDEN_DATA, ClientMessage as _, SMSG_AUTH_CHALLENGE, SMSG_WARDEN_DATA,
    ServerMessage as _, tokio_expect_client_message as world_expect_client_message,
    tokio_expect_server_message as world_expect_server_message,
};

/// Track loot responses only for targets that the bot explicitly activated for looting.
/// Game-object use can open a loot window just like a corpse loot request.
fn bot_loot_target_for_command(command: &GameplayCommand) -> Option<EntityId> {
    match command {
        GameplayCommand::Loot(target) | GameplayCommand::UseGameObject(target) => Some(*target),
        GameplayCommand::CastGameObject {
            spell,
            target,
            report_use: true,
        } if *spell == QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID => Some(*target),
        _ => None,
    }
}

fn gameobject_report_use_for_command(command: &GameplayCommand) -> Option<(EntityId, ClientFrame)> {
    match command {
        GameplayCommand::CastGameObject {
            target,
            report_use: true,
            ..
        } => Some((
            *target,
            ClientFrame {
                opcode: 0x0481,
                body: target.0.to_le_bytes().to_vec(),
            },
        )),
        _ => None,
    }
}

fn rejected_loot_target_without_response(
    target: Option<EntityId>,
    response_seen: bool,
) -> Option<EntityId> {
    if response_seen { None } else { target }
}

fn validate_near_teleport_ack(
    body: &[u8],
    expected: Option<(EntityId, u32)>,
    player_guid: Option<EntityId>,
    controlled_mover: Option<EntityId>,
) -> Result<(EntityId, u32, u32), &'static str> {
    let decoded =
        decode_near_teleport_ack(body).ok_or("malformed near-teleport acknowledgement")?;
    if expected != Some((decoded.0, decoded.1)) {
        return Err("near-teleport acknowledgement does not match pending correction");
    }
    if player_guid != Some(decoded.0) && controlled_mover != Some(decoded.0) {
        return Err("near-teleport acknowledgement mover is not controlled by this session");
    }
    Ok(decoded)
}

fn encode_body_hex(body: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(body.len().saturating_mul(2));
    for byte in body {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0F)] as char);
    }
    encoded
}

fn log_loot_wire_packet(account: &str, direction: &str, opcode: u32, body: &[u8]) {
    if !matches!(opcode, 0x0108 | 0x015D..=0x015F | 0x0160..=0x0163) {
        return;
    }
    let packet_guid = body
        .get(..8)
        .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
        .map(u64::from_le_bytes);
    let release_confirmed = (opcode == 0x0161).then(|| body.get(8).copied()).flatten();
    let body_hex = encode_body_hex(body);
    tracing::info!(
        account,
        direction,
        opcode = format_args!("0x{opcode:04X}"),
        body_len = body.len(),
        packet_guid,
        release_confirmed,
        body_hex,
        "loot protocol packet"
    );
}

#[cfg(test)]
mod auth_registry_tests {
    use super::*;

    fn proof(account: &str, client_seed: u32, server_seed: u32, key: &[u8; 40]) -> [u8; 20] {
        let mut hasher = Sha1::new();
        hasher.update(account.to_ascii_uppercase().as_bytes());
        hasher.update([0_u8; 4]);
        hasher.update(client_seed.to_le_bytes());
        hasher.update(server_seed.to_le_bytes());
        hasher.update(key);
        hasher.finalize().into()
    }

    #[tokio::test]
    async fn auth_registry_matches_world_login_to_the_right_key_behind_one_ip() {
        let registry = AuthRegistry::default();
        let ip = IpAddr::from([203, 0, 113, 7]);
        let first = [0x11; 40];
        let second = [0x22; 40];
        registry.put("TEST1", ip, first).await;
        registry.put("TEST2", ip, second).await;

        let matched = registry
            .verify("TEST1", ip, 3, 5, &proof("TEST1", 3, 5, &first))
            .await
            .unwrap();
        assert_eq!(matched.key, first);
        assert!(
            registry
                .verify("TEST1", ip, 3, 5, &proof("TEST1", 3, 5, &first))
                .await
                .is_none()
        );
        assert!(
            registry
                .verify("TEST1", ip, 3, 5, &proof("TEST1", 3, 5, &second))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn auth_registry_rejects_proof_from_a_different_peer_ip() {
        let registry = AuthRegistry::default();
        let ip = IpAddr::from([203, 0, 113, 7]);
        let other_ip = IpAddr::from([203, 0, 113, 8]);
        let key = [0x5a; 40];
        registry.put("TEST1", ip, key).await;

        assert!(
            registry
                .verify("TEST1", other_ip, 3, 5, &proof("TEST1", 3, 5, &key))
                .await
                .is_none()
        );
    }
}

#[derive(Clone, Debug)]
pub struct ProxyRuntimeConfig {
    pub auth_bind: String,
    pub world_bind: String,
    pub transparent_world_bind: String,
    pub advertise_host: String,
    pub upstream_auth_host: String,
    pub upstream_auth_port: u16,
    pub upstream_world_host: String,
    pub upstream_world_port: u16,
    pub realm_name: String,
    pub handshake_timeout: Duration,
    pub max_pre_auth_connections: usize,
    pub max_pre_auth_connections_per_ip: usize,
    pub log_dir: PathBuf,
    pub run_id: String,
    pub warden_client_image: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct ProxyAccountConfig {
    pub account: AccountId,
    pub lane: LaneId,
    pub worker: Option<WorkerGeneration>,
    pub account_name: String,
    pub password: String,
    pub character: Option<String>,
}

pub struct ManagedLane {
    pub config: ProxyAccountConfig,
    pub worker_rx: Option<mpsc::Receiver<WorkerToProxy>>,
    pub worker_tx: Option<mpsc::Sender<ProxyToWorker>>,
    pub supervisor_tx: mpsc::Sender<SupervisorCommand>,
    pub terminal_commands: mpsc::Receiver<TerminalBotCommand>,
}

pub struct TerminalBotCommand {
    pub text: String,
    pub reply: oneshot::Sender<Result<Vec<String>, String>>,
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MovementStamp {
    source: MovementVisualSource,
    session_id: u64,
    sequence: u64,
    ownership_generation: u64,
    movement_epoch: u64,
    route_epoch: u64,
    world_generation: u64,
}

#[derive(Clone)]
struct MovementMirror {
    stamp: MovementStamp,
    frame: crate::framing::ServerFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MovementVisualSource {
    Player,
    Bot,
    Server,
    ProxyHandoff,
}

impl MovementStamp {
    fn new(
        source: MovementVisualSource,
        session_id: u64,
        sequence: u64,
        owner: crate::ownership::OwnershipSnapshot,
        route_epoch: u64,
        world_generation: u64,
    ) -> Self {
        Self {
            source,
            session_id,
            sequence,
            ownership_generation: owner.generation.get(),
            movement_epoch: owner.movement_epoch.get(),
            route_epoch,
            world_generation,
        }
    }
}

fn movement_visual_is_current(
    stamp: MovementStamp,
    expected_source: MovementVisualSource,
    session_id: u64,
    ownership_generation: u64,
    movement_epoch: u64,
    route_epoch: u64,
    world_generation: u64,
    last_sequence: u64,
) -> bool {
    stamp.source == expected_source
        && stamp.session_id == session_id
        && stamp.ownership_generation == ownership_generation
        && stamp.movement_epoch == movement_epoch
        && stamp.route_epoch == route_epoch
        && stamp.world_generation == world_generation
        && stamp.sequence > last_sequence
}

fn invalidate_movement_mirror(epoch: &mut u64, updates: &watch::Sender<Option<MovementMirror>>) {
    *epoch = epoch.wrapping_add(1).max(1);
    updates.send_replace(None);
}

fn next_movement_sequence(sequence: &mut u64) -> u64 {
    *sequence = sequence.wrapping_add(1).max(1);
    *sequence
}

fn is_server_movement_visual_opcode(opcode: u32) -> bool {
    matches!(opcode, 0x00C7 | 0x00DD | 0x02AE)
}

fn movement_visual_source_is_owned(
    source: MovementVisualSource,
    owner: crate::ownership::OwnershipSnapshot,
) -> bool {
    match source {
        MovementVisualSource::Bot => owner.permits(crate::ownership::ClientKind::Bot),
        MovementVisualSource::ProxyHandoff => owner.mode == crate::ownership::ControlMode::Manual,
        MovementVisualSource::Player | MovementVisualSource::Server => false,
    }
}

fn is_movement_observation(observation: &ProtocolObservation) -> bool {
    matches!(
        observation,
        ProtocolObservation::PlayerPosition { .. } | ProtocolObservation::ControlledMover { .. }
    )
}

async fn fence_actor_movement(
    account: &AccountRuntime,
) -> Option<(wow_domain::MovementEpoch, u64)> {
    let (committed, receiver) = oneshot::channel();
    if account
        .session_tx
        .send(SessionMessage::WorldTransition { committed })
        .await
        .is_err()
    {
        return None;
    }
    receiver.await.ok()
}

async fn fence_world_movement(
    account: &AccountRuntime,
    world_generation: &mut u64,
    mirror_epoch: &mut u64,
    mirror_updates: &watch::Sender<Option<MovementMirror>>,
) -> bool {
    *world_generation = world_generation.wrapping_add(1).max(1);
    invalidate_movement_mirror(mirror_epoch, mirror_updates);
    match fence_actor_movement(account).await {
        Some((epoch, committed_world_generation)) => {
            *world_generation = committed_world_generation;
            tracing::info!(account=%account.config.account_name, world_generation=*world_generation, movement_epoch=epoch.get(), "movement fenced for world transition");
            true
        }
        None => {
            tracing::error!(account=%account.config.account_name, world_generation=*world_generation, "world transition movement fence was not committed");
            false
        }
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
struct AccountRuntime {
    config: ProxyAccountConfig,
    session_tx: mpsc::Sender<SessionMessage>,
    command_bus: broadcast::Sender<crate::configured_session::GameplayDispatch>,
    supervisor_tx: mpsc::Sender<SupervisorCommand>,
    mission_counter: Arc<AtomicU64>,
    headless_enabled: watch::Sender<bool>,
    headless_active: Arc<std::sync::atomic::AtomicBool>,
    assistance_armed: watch::Sender<bool>,
    ownership_state: watch::Sender<crate::ownership::OwnershipSnapshot>,
    visual_fence: broadcast::Sender<()>,
    movement_gate: Arc<tokio::sync::Mutex<()>>,
    player_worlds: Arc<Mutex<PlayerWorlds>>,
}

#[derive(Default)]
struct PlayerWorlds {
    next_id: u64,
    active: HashSet<u64>,
}

struct PlayerWorldHandoff {
    connection: u64,
    worlds: Arc<Mutex<PlayerWorlds>>,
    headless_enabled: watch::Sender<bool>,
}

impl PlayerWorldHandoff {
    fn begin(worlds: Arc<Mutex<PlayerWorlds>>, headless_enabled: watch::Sender<bool>) -> Self {
        let mut state = worlds
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.next_id = state
            .next_id
            .checked_add(1)
            .expect("player connection IDs exhausted");
        let connection = state.next_id;
        state.active.insert(connection);
        let _ = headless_enabled.send(false);
        drop(state);
        Self {
            connection,
            worlds,
            headless_enabled,
        }
    }
}

impl Drop for PlayerWorldHandoff {
    fn drop(&mut self) {
        let mut state = self
            .worlds
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active.remove(&self.connection);
        if state.active.is_empty() {
            let _ = self.headless_enabled.send(true);
        }
    }
}

#[derive(Clone, Default)]
struct AuthRegistry {
    keys: Arc<RwLock<HashMap<String, VecDeque<DownstreamAuthSession>>>>,
    next_generation: Arc<AtomicU64>,
}
#[derive(Clone)]
struct DownstreamAuthSession {
    peer_ip: IpAddr,
    generation: u64,
    created_at: tokio::time::Instant,
    key: [u8; 40],
}
impl AuthRegistry {
    async fn put(&self, account: &str, peer_ip: IpAddr, key: [u8; 40]) {
        const MAX_SESSIONS_PER_ACCOUNT: usize = 32;
        const SESSION_TTL: Duration = Duration::from_secs(45);
        let now = tokio::time::Instant::now();
        let generation = self.next_generation.fetch_add(1, Ordering::AcqRel).max(1);
        let mut keys = self.keys.write().await;
        let sessions = keys.entry(account.to_uppercase()).or_default();
        sessions.retain(|session| now.duration_since(session.created_at) <= SESSION_TTL);
        sessions.push_back(DownstreamAuthSession {
            peer_ip,
            generation,
            created_at: now,
            key,
        });
        while sessions.len() > MAX_SESSIONS_PER_ACCOUNT {
            sessions.pop_front();
        }
    }

    async fn verify(
        &self,
        account: &str,
        peer_ip: IpAddr,
        client_seed: u32,
        server_seed: u32,
        client_proof: &[u8; 20],
    ) -> Option<DownstreamAuthSession> {
        const SESSION_TTL: Duration = Duration::from_secs(45);
        let now = tokio::time::Instant::now();
        let mut keys = self.keys.write().await;
        let sessions = keys.get_mut(&account.to_uppercase())?;
        sessions.retain(|session| now.duration_since(session.created_at) <= SESSION_TTL);
        let index = sessions.iter().rposition(|session| {
            session.peer_ip == peer_ip
                && world_auth_proof_matches(
                    account,
                    client_seed,
                    server_seed,
                    &session.key,
                    client_proof,
                )
        })?;
        sessions.remove(index)
    }
}

fn world_auth_proof_matches(
    account: &str,
    client_seed: u32,
    server_seed: u32,
    session_key: &[u8; 40],
    client_proof: &[u8; 20],
) -> bool {
    let mut hasher = Sha1::new();
    hasher.update(account.to_ascii_uppercase().as_bytes());
    hasher.update([0_u8; 4]);
    hasher.update(client_seed.to_le_bytes());
    hasher.update(server_seed.to_le_bytes());
    hasher.update(session_key);
    let expected = hasher.finalize();
    expected
        .iter()
        .zip(client_proof)
        .fold(0_u8, |difference, (expected, actual)| {
            difference | (expected ^ actual)
        })
        == 0
}

#[derive(Clone)]
struct ActionLogManager {
    dir: PathBuf,
    sessions: Arc<tokio::sync::Mutex<HashMap<String, ActionLogSession>>>,
    operation: Arc<tokio::sync::Mutex<()>>,
}

struct ActionLogSession {
    path: PathBuf,
    tx: mpsc::Sender<ActionLogRecord>,
    failure: Arc<Mutex<Option<String>>>,
}

enum ActionLogRecord {
    Value(serde_json::Value),
    Stop(oneshot::Sender<Result<(), String>>),
}

static NEXT_ACTION_LOG_ID: AtomicU64 = AtomicU64::new(1);

impl ActionLogManager {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            operation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    async fn start(&self, account: &str, label: Option<&str>) -> Result<PathBuf> {
        let _operation = self.operation.lock().await;
        let _ = self.stop_inner(account).await?;
        prepare_action_log_directory(&self.dir)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let unique = NEXT_ACTION_LOG_ID.fetch_add(1, Ordering::Relaxed);
        let safe = account
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>();
        let path = self.dir.join(format!(
            "action-{safe}-{stamp}-{}-{unique}.jsonl",
            std::process::id(),
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = options.open(&path)?;
        let start = serde_json::json!({
            "event": "start",
            "account": account,
            "label": label,
            "unix_s": stamp,
        });
        writeln!(file, "{start}")?;
        file.flush()?;
        let (tx, mut rx) = mpsc::channel::<ActionLogRecord>(4096);
        let failure = Arc::new(Mutex::new(None));
        let writer_failure = failure.clone();
        tokio::task::spawn_blocking(move || {
            while let Some(record) = rx.blocking_recv() {
                match record {
                    ActionLogRecord::Value(value) => {
                        if let Err(error) = writeln!(file, "{value}").and_then(|_| file.flush()) {
                            *writer_failure
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                Some(error.to_string());
                            break;
                        }
                    }
                    ActionLogRecord::Stop(reply) => {
                        let result = file.flush().map_err(|error| error.to_string());
                        let _ = reply.send(result);
                        break;
                    }
                }
            }
        });
        self.sessions.lock().await.insert(
            account.to_uppercase(),
            ActionLogSession {
                path: path.clone(),
                tx,
                failure,
            },
        );
        Ok(path)
    }

    async fn stop(&self, account: &str) -> Result<Option<(PathBuf, Option<String>)>> {
        let _operation = self.operation.lock().await;
        self.stop_inner(account).await
    }

    async fn stop_inner(&self, account: &str) -> Result<Option<(PathBuf, Option<String>)>> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.remove(&account.to_uppercase()) {
            let failure = session
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if failure.is_some() {
                return Ok(Some((session.path, failure)));
            }
            let (reply, result) = oneshot::channel();
            let record = serde_json::json!({"event": "stop"});
            if session
                .tx
                .send(ActionLogRecord::Value(record))
                .await
                .is_err()
            {
                return Ok(Some((
                    session.path,
                    Some("Action Log writer stopped before capture completed".into()),
                )));
            }
            if session.tx.send(ActionLogRecord::Stop(reply)).await.is_err() {
                return Ok(Some((
                    session.path,
                    Some("Action Log writer stopped before flush".into()),
                )));
            }
            let flush = result.await.context("Action Log writer result was lost")?;
            if let Err(error) = flush {
                return Ok(Some((session.path, Some(error))));
            }
            return Ok(Some((session.path, failure)));
        }
        Ok(None)
    }

    async fn status(&self, account: &str) -> Option<(PathBuf, Option<String>)> {
        self.sessions
            .lock()
            .await
            .get(&account.to_uppercase())
            .map(|session| {
                (
                    session.path.clone(),
                    session
                        .failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone(),
                )
            })
    }

    async fn mark(&self, account: &str, label: Option<&str>) -> Result<bool> {
        let sessions = self.sessions.lock().await;
        let Some(session) = sessions.get(&account.to_uppercase()) else {
            return Ok(false);
        };
        let record = serde_json::json!({"event": "mark", "label": label.unwrap_or("mark")});
        match session.tx.try_send(ActionLogRecord::Value(record)) {
            Ok(()) => Ok(true),
            Err(error) => {
                *session
                    .failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(format!("Action Log queue unavailable: {error}"));
                Err(anyhow::anyhow!("Action Log queue is unavailable"))
            }
        }
    }

    async fn packet(&self, account: &str, stream: &str, direction: &str, opcode: u32, body: &[u8]) {
        let sessions = self.sessions.lock().await;
        let Some(session) = sessions.get(&account.to_uppercase()) else {
            return;
        };
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let fingerprint = body.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        const MAX_LOGGED_PACKET_BODY: usize = 4096;
        let body_hex = encode_body_hex(&body[..body.len().min(MAX_LOGGED_PACKET_BODY)]);
        let redaction_reason = action_log_redaction_reason(direction, opcode);
        let record = serde_json::json!({
            "event": "packet",
            "unix_ms": unix_ms,
            "stream": stream,
            "direction": direction,
            "opcode": opcode,
            "body_len": body.len(),
            "body_truncated": redaction_reason.is_none() && body.len() > MAX_LOGGED_PACKET_BODY,
            "fingerprint": format!("{fingerprint:016X}"),
            "body_hex": if redaction_reason.is_none() { Some(body_hex) } else { None },
            "redaction_reason": redaction_reason,
        });
        if let Err(error) = session.tx.try_send(ActionLogRecord::Value(record)) {
            *session
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(format!("Action Log queue unavailable: {error}"));
            tracing::warn!(
                account,
                "Action Log dropped a packet record because its bounded writer queue is full"
            );
        }
    }

    async fn position(
        &self,
        account: &str,
        source: &str,
        position: WorldPosition,
        mover: Option<EntityId>,
        opcode: Option<u32>,
        flags: Option<u32>,
        client_time: Option<u32>,
    ) {
        if !position.point.is_finite() || !position.orientation.is_finite() {
            return;
        }
        let sessions = self.sessions.lock().await;
        let Some(session) = sessions.get(&account.to_uppercase()) else {
            return;
        };
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let record = serde_json::json!({
            "event": "position",
            "unix_ms": unix_ms,
            "source": source,
            "map": position.map,
            "x": position.point.x,
            "y": position.point.y,
            "z": position.point.z,
            "orientation": position.orientation,
            "mover": mover.map(|id| id.0),
            "opcode": opcode,
            "flags": flags,
            "client_time": client_time,
        });
        if let Err(error) = session.tx.try_send(ActionLogRecord::Value(record)) {
            *session
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(format!("Action Log queue unavailable: {error}"));
            tracing::warn!(
                account,
                "Action Log dropped a position record because its bounded writer queue is full"
            );
        }
    }
}

fn action_log_redaction_reason(direction: &str, opcode: u32) -> Option<&'static str> {
    match (direction, opcode) {
        ("C2S", 0x0095) | ("S2C", 0x0096 | 0x03B3) => Some("chat payload redacted"),
        ("C2S", opcode) if opcode == CMSG_WARDEN_DATA::OPCODE => Some("Warden payload redacted"),
        ("S2C", opcode) if opcode == SMSG_WARDEN_DATA::OPCODE => Some("Warden payload redacted"),
        _ => None,
    }
}

fn prepare_action_log_directory(path: &Path) -> Result<()> {
    wow_infra::security::paths::reject_symlink_path(path)
        .context("Action Log path cannot traverse a symbolic link")?;
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Action Log path must be a real directory"
        );
    } else {
        std::fs::create_dir_all(path)?;
    }
    wow_infra::security::paths::reject_symlink_path(path)
        .context("Action Log path cannot traverse a symbolic link")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[derive(Clone)]
struct SharedRuntime {
    config: Arc<ProxyRuntimeConfig>,
    accounts: Arc<HashMap<String, AccountRuntime>>,
    auth: AuthRegistry,
    admission: AdmissionController,
    action_logs: ActionLogManager,
}

pub async fn run(config: ProxyRuntimeConfig, lanes: Vec<ManagedLane>) -> Result<()> {
    let diagnostic_files = lanes.iter().flat_map(|lane| {
        DiagnosticStream::PROXY.into_iter().map(|stream| {
            (
                stream,
                lane.config.lane,
                diagnostic_path(&config.log_dir, stream, lane.config.lane),
            )
        })
    });
    let diagnostics = DiagnosticLogger::new(config.run_id.clone(), diagnostic_files);
    let mut session_actors = Vec::new();
    let mut accounts = HashMap::new();
    let mut terminal_command_receivers = Vec::new();
    for lane in lanes {
        for stream in DiagnosticStream::PROXY {
            diagnostics.record(
                stream,
                lane.config.lane,
                "run_started",
                serde_json::json!({
                    "process_id": std::process::id(),
                }),
            );
        }
        let (session_tx, session_rx) = mpsc::channel(256);
        let (command_tx, mut command_rx) = mpsc::channel(256);
        let (command_bus, _) = broadcast::channel(256);
        let (headless_enabled, _) = watch::channel(true);
        let (assistance_armed, _) = watch::channel(false);
        let (visual_fence, _) = broadcast::channel(16);
        let headless_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let state =
            ConfiguredSessionState::new(lane.config.account, lane.config.lane, lane.config.worker);
        let (ownership_state, _) = watch::channel(state.ownership.snapshot());
        let movement_gate = Arc::new(tokio::sync::Mutex::new(()));
        let supervisor_for_runtime = lane.supervisor_tx.clone();
        session_actors.push(tokio::spawn(
            ConfiguredSessionActor {
                state,
                rx: session_rx,
                worker_tx: lane.worker_tx.clone(),
                upstream_tx: command_tx.clone(),
                supervisor_tx: lane.supervisor_tx,
                diagnostics: diagnostics.clone(),
                assistance_armed: assistance_armed.clone(),
                ownership_state: ownership_state.clone(),
                movement_gate: movement_gate.clone(),
                world_generation: 1,
                movement_sequence: 0,
                last_worker_movement_sequence: None,
            }
            .run(),
        ));
        if let Some(mut worker_rx) = lane.worker_rx {
            let session = session_tx.clone();
            tokio::spawn(async move {
                while let Some(message) = worker_rx.recv().await {
                    if session.send(SessionMessage::Worker(message)).await.is_err() {
                        break;
                    }
                }
            });
        }
        let bus = command_bus.clone();
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                let _ = bus.send(command);
            }
        });
        let account_key = lane.config.account_name.to_uppercase();
        accounts.insert(
            account_key.clone(),
            AccountRuntime {
                config: lane.config,
                session_tx,
                command_bus,
                supervisor_tx: supervisor_for_runtime,
                mission_counter: Arc::new(AtomicU64::new(1)),
                headless_enabled,
                headless_active,
                assistance_armed,
                ownership_state,
                visual_fence,
                movement_gate,
                player_worlds: Arc::new(Mutex::new(PlayerWorlds::default())),
            },
        );
        terminal_command_receivers.push((account_key, lane.terminal_commands));
    }
    let action_logs = ActionLogManager::new(config.log_dir.clone());
    let admission = AdmissionController::new(AdmissionLimits {
        max_total: config.max_pre_auth_connections,
        max_per_ip: config.max_pre_auth_connections_per_ip,
    });
    let shared = SharedRuntime {
        config: Arc::new(config),
        accounts: Arc::new(accounts),
        auth: AuthRegistry::default(),
        admission,
        action_logs,
    };
    let mut terminal_tasks = Vec::new();
    for (account_key, mut receiver) in terminal_command_receivers {
        let account = shared.accounts[&account_key].clone();
        terminal_tasks.push(tokio::spawn(async move {
            while let Some(request) = receiver.recv().await {
                let reply = execute_terminal_bot_command(&account, &request.text).await;
                let _ = request.reply.send(reply);
            }
        }));
    }
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let auth_ready = ready_tx.clone();
    let world_ready = ready_tx.clone();
    let auth = tokio::spawn(serve_player_auth(shared.clone(), auth_ready));
    let world = tokio::spawn(serve_configured_world(shared.clone(), world_ready));
    let transparent = tokio::spawn(serve_transparent_world(shared.clone(), ready_tx));
    let mut auth = auth;
    let mut world = world;
    let mut transparent = transparent;
    if let Err(error) =
        wait_for_listener_readiness(&mut ready_rx, &mut auth, &mut world, &mut transparent).await
    {
        stop_listener(&mut auth).await;
        stop_listener(&mut world).await;
        stop_listener(&mut transparent).await;
        for task in terminal_tasks {
            task.abort();
            let _ = task.await;
        }
        for account in shared.accounts.values() {
            let _ = account.session_tx.send(SessionMessage::Shutdown).await;
        }
        for actor in session_actors {
            let _ = actor.await;
        }
        diagnostics.flush();
        return Err(error);
    }
    tracing::info!("all proxy listeners are ready");
    let mut headless_tasks = Vec::new();
    for account in shared
        .accounts
        .values()
        .filter(|account| account.config.worker.is_some())
        .cloned()
    {
        let shared_for_headless = shared.clone();
        headless_tasks.push(tokio::spawn(async move {
            headless_session_manager(shared_for_headless, account).await;
        }));
    }
    let runtime_result = tokio::select! {
        r = &mut auth => r.context("auth listener task failed").and_then(|result| result),
        r = &mut world => r.context("configured world listener task failed").and_then(|result| result),
        r = &mut transparent => r.context("transparent world listener task failed").and_then(|result| result),
        _ = tokio::signal::ctrl_c() => Ok(()),
    };
    stop_listener(&mut auth).await;
    stop_listener(&mut world).await;
    stop_listener(&mut transparent).await;
    for task in terminal_tasks {
        task.abort();
        let _ = task.await;
    }
    for task in headless_tasks {
        task.abort();
        let _ = task.await;
    }
    for account in shared.accounts.values() {
        let _ = account.session_tx.send(SessionMessage::Shutdown).await;
    }
    for actor in session_actors {
        if let Err(error) = actor.await {
            tracing::warn!(%error, "configured session actor stopped unexpectedly during proxy shutdown");
        }
    }
    diagnostics.flush();
    runtime_result
}

async fn wait_for_listener_readiness(
    ready_rx: &mut mpsc::UnboundedReceiver<()>,
    auth: &mut tokio::task::JoinHandle<Result<()>>,
    world: &mut tokio::task::JoinHandle<Result<()>>,
    transparent: &mut tokio::task::JoinHandle<Result<()>>,
) -> Result<()> {
    for _ in 0..3 {
        tokio::select! {
            ready = ready_rx.recv() => { ready.context("proxy listener readiness channel closed")?; }
            result = &mut *auth => { result.context("auth listener task failed")??; bail!("auth listener stopped before startup completed"); }
            result = &mut *world => { result.context("configured world listener task failed")??; bail!("configured world listener stopped before startup completed"); }
            result = &mut *transparent => { result.context("transparent world listener task failed")??; bail!("transparent world listener stopped before startup completed"); }
            _ = tokio::signal::ctrl_c() => bail!("proxy startup cancelled before listeners were ready"),
        }
    }
    Ok(())
}

async fn stop_listener(listener: &mut tokio::task::JoinHandle<Result<()>>) {
    if !listener.is_finished() {
        listener.abort();
        let _ = listener.await;
    }
}

async fn serve_player_auth(shared: SharedRuntime, ready: mpsc::UnboundedSender<()>) -> Result<()> {
    let listener = TcpListener::bind(&shared.config.auth_bind).await?;
    let _ = ready.send(());
    tracing::info!(bind=%shared.config.auth_bind, "player auth listener ready");
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => accepted?,
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = completed { tracing::warn!(%error, "player auth connection task failed"); }
                continue;
            }
        };
        let permit = match shared.admission.try_acquire(peer.ip()) {
            Ok(p) => p,
            Err(reason) => {
                tracing::warn!(%peer, reason, "rejected pre-auth connection");
                continue;
            }
        };
        let shared = shared.clone();
        connections.spawn(async move {
            let result = handle_auth(shared.clone(), stream, peer.ip(), permit).await;
            match result {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(%peer, error=%format_args!("{e:#}"), "single-port player connection failed")
                }
            }
        });
    }
}

async fn serve_configured_world(
    shared: SharedRuntime,
    ready: mpsc::UnboundedSender<()>,
) -> Result<()> {
    let listener = TcpListener::bind(&shared.config.world_bind).await?;
    let _ = ready.send(());
    tracing::info!(bind=%shared.config.world_bind, "configured player world listener ready");
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => accepted?,
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = completed { tracing::warn!(%error, "configured world connection task failed"); }
                continue;
            }
        };
        let permit = match shared.admission.try_acquire(peer.ip()) {
            Ok(permit) => permit,
            Err(reason) => {
                tracing::warn!(%peer, reason, "rejected configured world connection");
                continue;
            }
        };
        let shared = shared.clone();
        connections.spawn(async move {
            let result = configured_world(shared.clone(), stream, peer.ip(), permit).await;
            if let Err(error) = result {
                tracing::warn!(%peer, error=%format_args!("{error:#}"), "configured world connection failed");
            }
        });
    }
}

async fn serve_transparent_world(
    shared: SharedRuntime,
    ready: mpsc::UnboundedSender<()>,
) -> Result<()> {
    let listener = TcpListener::bind(&shared.config.transparent_world_bind).await?;
    let _ = ready.send(());
    tracing::info!(bind=%shared.config.transparent_world_bind, "transparent world listener ready");
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let (mut downstream, peer) = tokio::select! {
            accepted = listener.accept() => accepted?,
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = completed { tracing::warn!(%error, "transparent world connection task failed"); }
                continue;
            }
        };
        let permit = match shared.admission.try_acquire(peer.ip()) {
            Ok(permit) => permit,
            Err(reason) => {
                tracing::warn!(%peer, reason, "rejected transparent world connection");
                continue;
            }
        };
        let shared = shared.clone();
        connections.spawn(async move {
            let addr = advertised(
                &shared.config.upstream_world_host,
                shared.config.upstream_world_port,
            );
            let result = async {
                let mut upstream = TcpStream::connect(&addr)
                    .await
                    .with_context(|| format!("connect upstream world {addr}"))?;
                let mut initial = [0_u8; 1];
                let received = tokio::time::timeout(shared.config.handshake_timeout, upstream.peek(&mut initial))
                    .await
                    .map_err(|_| anyhow::anyhow!("transparent world handshake timed out"))??;
                if received == 0 { bail!("upstream world closed before its first handshake byte"); }
                drop(permit);
                let (up, down) = transparent_relay(&mut downstream, &mut upstream).await?;
                tracing::debug!(%peer, up, down, upstream=%addr, "transparent world relay ended");
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(%peer, error=%format_args!("{error:#}"), "transparent world connection failed");
            }
        });
    }
}

struct StockChallenge {
    account: String,
    raw: Vec<u8>,
}
async fn read_stock_challenge(stream: &mut TcpStream) -> Result<StockChallenge> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await?;
    if prefix[0] != 0 || prefix[1] != 8 {
        bail!("expected WotLK 3.3.5a login challenge");
    }
    let size = usize::from(u16::from_le_bytes([prefix[2], prefix[3]]));
    if !(30..=4096).contains(&size) {
        bail!("invalid login challenge length {size}");
    }
    let mut body = vec![0_u8; size];
    stream.read_exact(&mut body).await?;
    let n = usize::from(*body.get(29).context("missing account length")?);
    let account =
        std::str::from_utf8(body.get(30..30 + n).context("truncated account name")?)?.to_owned();
    let mut raw = prefix.to_vec();
    raw.extend_from_slice(&body);
    Ok(StockChallenge { account, raw })
}

async fn handle_auth(
    shared: SharedRuntime,
    mut downstream: TcpStream,
    peer_ip: IpAddr,
    permit: crate::auth::limits::AdmissionPermit,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + shared.config.handshake_timeout;
    let challenge = tokio::time::timeout_at(deadline, read_stock_challenge(&mut downstream))
        .await
        .context("downstream auth challenge timed out")??;
    let key = challenge.account.to_uppercase();
    if let Some(account) = shared.accounts.get(&key) {
        let upstream = tokio::time::timeout_at(
            deadline,
            upstream_login(&upstream_config(&shared, account), &account.config.password),
        )
        .await
        .context("upstream auth handshake timed out")??;
        let authenticated = tokio::time::timeout_at(
            deadline,
            crate::auth::login::terminate_configured_downstream(
                &mut downstream,
                &ConfiguredCredential {
                    account: account.config.account_name.clone(),
                    password: account.config.password.clone(),
                },
            ),
        )
        .await
        .context("downstream auth proof timed out")??;
        shared
            .auth
            .put(&key, peer_ip, authenticated.session_key)
            .await;
        drop(permit);
        serve_configured_realms(&shared, &mut downstream, &upstream.realm_name, peer_ip).await
    } else {
        transparent_auth(shared, challenge, downstream, peer_ip, permit, deadline).await
    }
}

fn upstream_config(shared: &SharedRuntime, account: &AccountRuntime) -> UpstreamAuthConfig {
    UpstreamAuthConfig {
        host: shared.config.upstream_auth_host.clone(),
        port: shared.config.upstream_auth_port,
        realm_name: shared.config.realm_name.clone(),
        world_host: shared.config.upstream_world_host.clone(),
        world_port: shared.config.upstream_world_port,
        account: account.config.account_name.clone(),
    }
}

async fn serve_configured_realms(
    shared: &SharedRuntime,
    stream: &mut TcpStream,
    realm_name: &str,
    peer_ip: IpAddr,
) -> Result<()> {
    let address = downstream_realm_address(&shared, peer_ip)?;
    tracing::info!(%peer_ip, %address, "selected downstream realm address");
    while tokio_expect_client_message::<CMD_REALM_LIST_Client, _>(&mut *stream)
        .await
        .is_ok()
    {
        CMD_REALM_LIST_Server {
            realms: vec![Realm {
                realm_type: RealmType::PlayerVsEnvironment,
                locked: false,
                flag: Default::default(),
                name: realm_name.to_owned(),
                address: address.clone(),
                population: Default::default(),
                number_of_characters_on_realm: 1,
                category: RealmCategory::One,
                realm_id: 1,
            }],
        }
        .tokio_write(&mut *stream)
        .await?;
    }
    Ok(())
}

async fn transparent_auth(
    shared: SharedRuntime,
    challenge: StockChallenge,
    mut downstream: TcpStream,
    peer_ip: IpAddr,
    permit: crate::auth::limits::AdmissionPermit,
    deadline: tokio::time::Instant,
) -> Result<()> {
    let addr = advertised(
        &shared.config.upstream_auth_host,
        shared.config.upstream_auth_port,
    );
    let mut upstream = tokio::time::timeout_at(deadline, TcpStream::connect(&addr))
        .await
        .context("transparent upstream auth connect timed out")??;
    tokio::time::timeout_at(deadline, upstream.write_all(&challenge.raw))
        .await
        .context("transparent challenge write timed out")??;
    // Relay challenge and proof without deriving the unknown account session key.
    let response = tokio::time::timeout_at(
        deadline,
        tokio_expect_server_message_protocol::<
            wow_login_messages::version_8::CMD_AUTH_LOGON_CHALLENGE_Server,
            _,
        >(&mut upstream, ProtocolVersion::Eight),
    )
    .await
    .context("transparent auth challenge response timed out")??;
    tokio::time::timeout_at(deadline, response.tokio_write(&mut downstream))
        .await
        .context("transparent challenge relay timed out")??;
    let proof = tokio::time::timeout_at(
        deadline,
        tokio_expect_client_message_protocol::<
            wow_login_messages::version_8::CMD_AUTH_LOGON_PROOF_Client,
            _,
        >(&mut downstream, ProtocolVersion::Eight),
    )
    .await
    .context("transparent downstream proof timed out")??;
    tokio::time::timeout_at(deadline, proof.tokio_write(&mut upstream))
        .await
        .context("transparent proof relay timed out")??;
    let response = tokio::time::timeout_at(
        deadline,
        tokio_expect_server_message_protocol::<
            wow_login_messages::version_8::CMD_AUTH_LOGON_PROOF_Server,
            _,
        >(&mut upstream, ProtocolVersion::Eight),
    )
    .await
    .context("transparent upstream proof response timed out")??;
    tokio::time::timeout_at(deadline, response.tokio_write(&mut downstream))
        .await
        .context("transparent proof response timed out")??;
    let authenticated = matches!(
        &response,
        wow_login_messages::version_8::CMD_AUTH_LOGON_PROOF_Server::Success { .. }
    );
    if !authenticated {
        return Ok(());
    }
    drop(permit);
    loop {
        let request = match tokio_expect_client_message_protocol::<CMD_REALM_LIST_Client, _>(
            &mut downstream,
            ProtocolVersion::Eight,
        )
        .await
        {
            Ok(v) => v,
            Err(_) => break,
        };
        request.tokio_write(&mut upstream).await?;
        let mut realms = tokio_expect_server_message_protocol::<CMD_REALM_LIST_Server, _>(
            &mut upstream,
            ProtocolVersion::Eight,
        )
        .await?;
        for realm in &mut realms.realms {
            if realm.name.eq_ignore_ascii_case(&shared.config.realm_name) {
                realm.address = advertised(
                    realm_advertise_host(
                        peer_ip,
                        &shared.config.upstream_world_host,
                        &shared.config.advertise_host,
                    ),
                    port_of(&shared.config.transparent_world_bind)?,
                );
            }
        }
        realms.tokio_write(&mut downstream).await?;
    }
    Ok(())
}

fn downstream_realm_address(shared: &SharedRuntime, peer_ip: IpAddr) -> Result<String> {
    Ok(advertised(
        realm_advertise_host(
            peer_ip,
            &shared.config.upstream_world_host,
            &shared.config.advertise_host,
        ),
        port_of(&shared.config.world_bind)?,
    ))
}

fn realm_advertise_host<'a>(peer_ip: IpAddr, local_host: &'a str, remote_host: &'a str) -> &'a str {
    if is_local_network(peer_ip) {
        // The upstream world host is the LAN address for the colocated
        // AzerothCore and proxy setup. Remote clients use the public proxy host.
        local_host
    } else {
        remote_host
    }
}

fn is_local_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

async fn configured_world(
    shared: SharedRuntime,
    mut downstream: TcpStream,
    peer_ip: IpAddr,
    permit: crate::auth::limits::AdmissionPermit,
) -> Result<()> {
    let handshake_deadline = tokio::time::Instant::now() + shared.config.handshake_timeout;
    let seed = ProofSeed::new();
    SMSG_AUTH_CHALLENGE {
        unknown1: 1,
        server_seed: seed.seed(),
        seed: [0_u8; 32],
    }
    .tokio_write_unencrypted_server(&mut downstream)
    .await?;
    let auth = tokio::time::timeout_at(
        handshake_deadline,
        world_expect_client_message::<CMSG_AUTH_SESSION, _>(&mut downstream),
    )
    .await
    .map_err(|_| anyhow::anyhow!("configured world auth session timed out"))??;
    let account_name = auth.username.to_uppercase();
    let account = shared
        .accounts
        .get(&account_name)
        .with_context(|| format!("world account {account_name} is not configured"))?
        .clone();
    let downstream_session = shared
        .auth
        .verify(
            &account_name,
            peer_ip,
            auth.client_seed,
            seed.seed(),
            &auth.client_proof,
        )
        .await
        .context("no matching fresh downstream auth proof for configured world connection")?;
    tracing::debug!(account=%account_name, auth_generation=downstream_session.generation, "matched configured world login to auth session");
    let downstream_key = downstream_session.key;
    let handoff = PlayerWorldHandoff::begin(
        account.player_worlds.clone(),
        account.headless_enabled.clone(),
    );
    tokio::time::timeout_at(
        handshake_deadline,
        wait_for_headless_state(&account, false, Duration::from_secs(6)),
    )
    .await
    .context("headless session handoff timed out")??;
    let normalized = NormalizedString::new(&account_name)?;
    let downstream_crypto = seed.into_server_header_crypto(
        &normalized,
        downstream_key,
        auth.client_proof,
        auth.client_seed,
    )?;
    let (mut down_enc, mut down_dec) = downstream_crypto.split();

    let upstream_login = tokio::time::timeout_at(
        handshake_deadline,
        upstream_login(
            &upstream_config(&shared, &account),
            &account.config.password,
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("upstream player authentication timed out"))??;
    let addr = advertised(&upstream_login.world_host, upstream_login.world_port);
    let mut upstream = tokio::time::timeout_at(handshake_deadline, TcpStream::connect(&addr))
        .await
        .map_err(|_| anyhow::anyhow!("upstream world connect timed out"))?
        .with_context(|| format!("connect upstream world {addr}"))?;
    let challenge = tokio::time::timeout_at(
        handshake_deadline,
        world_expect_server_message::<SMSG_AUTH_CHALLENGE, _>(&mut upstream),
    )
    .await
    .map_err(|_| anyhow::anyhow!("upstream world challenge timed out"))??;
    let up_seed = ProofSeed::new();
    let client_seed = up_seed.seed();
    let (proof, up_crypto) = up_seed.into_client_header_crypto(
        &normalized,
        upstream_login.session_key,
        challenge.server_seed,
    );
    let connection = handoff.connection;
    let (ready, paused) = oneshot::channel();
    account
        .session_tx
        .send(SessionMessage::PreparePlayerLogin { connection, ready })
        .await
        .context("configured session actor is unavailable during player login")?;
    if !tokio::time::timeout_at(handshake_deadline, paused)
        .await
        .context("configured session actor handoff timed out")?
        .context("configured session actor stopped before player login handoff")?
    {
        bail!("failed to pause configured bot lane before player login");
    }
    CMSG_AUTH_SESSION {
        client_build: 12340,
        login_server_id: 0,
        username: account_name.clone(),
        login_server_type: 0,
        client_seed,
        region_id: 0,
        battleground_id: 0,
        realm_id: upstream_login.realm_id,
        dos_response: 0,
        client_proof: proof,
        addon_info: auth.addon_info,
    }
    .tokio_write_unencrypted_client(&mut upstream)
    .await?;
    drop(permit);
    let (mut up_enc, mut up_dec) = up_crypto.split();
    let mut warden = WardenBridge::new(&upstream_login.session_key, &downstream_key);

    let mut commands = account.command_bus.subscribe();
    let assistance_armed = account.assistance_armed.subscribe();
    let ownership_state = account.ownership_state.subscribe();
    let mut visual_fence = account.visual_fence.subscribe();
    account
        .session_tx
        .send(SessionMessage::UpstreamConnected(true))
        .await
        .ok();
    tracing::info!(account=%account_name, upstream=%addr, "configured player world bridge attached");

    let mut object_observer =
        ObjectObservationRuntime::new().context("initialize Tentacli object observer")?;
    let mut player_guid: Option<EntityId> = None;
    let mut canonical_position: Option<WorldPosition> = None;
    let mut controlled_mover: Option<EntityId> = None;
    let mut controlled_position: Option<WorldPosition> = None;
    let mut controlled_flags: u32 = 0;
    let mut canonical_flags: u32 = 0;
    let mut movement_clock = MovementClock::default();
    let mut server_motion = ServerMotionProjection::default();
    let mut projected_mover: Option<EntityId> = None;
    static NEXT_WORLD_SESSION: AtomicU64 = AtomicU64::new(1);
    let movement_session_id = NEXT_WORLD_SESSION.fetch_add(1, Ordering::Relaxed).max(1);
    let mut movement_world_generation = 1_u64;
    let mut last_bot_visual: Option<(WorldPosition, EntityId, u32, std::time::Instant)> = None;
    let mut last_bot_movement_at: Option<std::time::Instant> = None;
    let mut last_bot_command_sequence: Option<(u64, u64, u64)> = None;
    let mut pending_near_teleport_ack: Option<(EntityId, u32)> = None;
    let mut pending_near_teleport_position: Option<(EntityId, WorldPosition)> = None;
    let mut pending_movement_corrections: VecDeque<MovementCorrectionExpectation> = VecDeque::new();
    let (movement_mirror_tx, mut movement_mirror_rx) = watch::channel(None::<MovementMirror>);
    let mut movement_mirror_sequence = 0_u64;
    let mut movement_mirror_epoch = 1_u64;
    let mut last_mirrored_sequence = 0_u64;
    let mut last_player_sequence = 0_u64;
    let mut last_server_sequence = 0_u64;
    let mut bot_loot_target: Option<EntityId> = None;
    let mut bot_loot_response_seen = false;
    let mut last_bot_cast: Option<(u32, Option<EntityId>, std::time::Instant)> = None;
    let (mut dr, mut dw) = downstream.into_split();
    let (mut ur, mut uw) = upstream.into_split();
    if !fence_world_movement(
        &account,
        &mut movement_world_generation,
        &mut movement_mirror_epoch,
        &movement_mirror_tx,
    )
    .await
    {
        bail!("could not commit initial configured-world movement fence");
    }
    let result: Result<()> = loop {
        tokio::select! {
            client = read_client_frame(&mut dr, &mut down_dec) => {
                let mut frame = match client { Ok(v)=>v, Err(e)=>break Err(e) };
                shared.action_logs.packet(
                    &account_name,
                    "player",
                    "C2S",
                    u32::from(frame.opcode),
                    &frame.body,
                ).await;
                if frame.opcode == 0x0391
                    && let Some((counter, client_time_ms)) = parse_time_sync_response(&frame.body)
                {
                    movement_clock.observe(client_time_ms);
                    tracing::debug!(account=%account_name, counter, client_time_ms, "observed client movement time synchronization response");
                }
                if frame.opcode == 0x003D {
                    if let Some(bytes) = frame.body.get(0..8) {
                        let guid = EntityId(u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])));
                        player_guid = Some(guid);
                        object_observer.set_player_guid(guid);
                        tracing::info!(account=%account_name, ?guid, "captured configured character GUID from CMSG_PLAYER_LOGIN");
                    }
                }
                let mut suppress_client_movement_upstream = false;
                let mut player_movement_stamp = None;
                let mut proxy_handoff_stamp = None;
                if u32::from(frame.opcode) == 0x00C7 {
                    let expected_ack = pending_near_teleport_ack;
                    let decoded_ack = validate_near_teleport_ack(
                        &frame.body,
                        expected_ack,
                        player_guid,
                        controlled_mover,
                    );
                    pending_near_teleport_ack = None;
                    if let Err(reason) = decoded_ack {
                        tracing::warn!(account=%account_name, source="server_correction", sequence=movement_mirror_sequence, session=movement_session_id, world_generation=movement_world_generation, opcode=frame.opcode, expected_ack=?expected_ack, ownership_generation=?ownership_state.borrow().generation, movement_epoch=?ownership_state.borrow().movement_epoch, reason, "dropped unmatched or malformed near-teleport acknowledgement");
                        continue;
                    }
                    if let (Ok((mover, _, client_time)), Some((expected_mover, position))) =
                        (decoded_ack, pending_near_teleport_position.take())
                        && mover == expected_mover
                    {
                        movement_clock.observe(client_time);
                        if controlled_mover == Some(mover) {
                            controlled_position = Some(position);
                        } else {
                            canonical_position = Some(position);
                        }
                        server_motion.reset_to(mover, position, client_time);
                        projected_mover = Some(mover);
                    }
                    let sequence = next_movement_sequence(&mut movement_mirror_sequence);
                    proxy_handoff_stamp = Some(MovementStamp::new(
                        MovementVisualSource::ProxyHandoff,
                        movement_session_id,
                        sequence,
                        *ownership_state.borrow(),
                        movement_mirror_epoch,
                        movement_world_generation,
                    ));
                    tracing::info!(account=%account_name, source="server_correction", sequence=movement_mirror_sequence, opcode=frame.opcode, mover=?decoded_ack.ok().map(|value| value.0), session=movement_session_id, world_generation=movement_world_generation, ownership_generation=?ownership_state.borrow().generation, movement_epoch=?ownership_state.borrow().movement_epoch, reason="matched pending correction", "accepted validated near-teleport acknowledgement");
                }
                if is_movement_correction_ack_opcode(u32::from(frame.opcode)) && u32::from(frame.opcode) != 0x00C7 {
                    let matched = pending_movement_corrections.iter().position(|expected| {
                        expected.ack_opcode == u32::from(frame.opcode)
                            && (player_guid == Some(expected.mover) || controlled_mover == Some(expected.mover))
                            && validate_movement_correction_ack(u32::from(frame.opcode), &frame.body, *expected).is_ok()
                    });
                    if let Some(index) = matched {
                        let expected = pending_movement_corrections.remove(index).expect("matched correction index exists");
                        let sequence = next_movement_sequence(&mut movement_mirror_sequence);
                        proxy_handoff_stamp = Some(MovementStamp::new(
                            MovementVisualSource::ProxyHandoff,
                            movement_session_id,
                            sequence,
                            *ownership_state.borrow(),
                            movement_mirror_epoch,
                            movement_world_generation,
                        ));
                        tracing::info!(account=%account_name, source="server_correction", sequence=movement_mirror_sequence, opcode=frame.opcode, mover=?expected.mover, session=movement_session_id, world_generation=movement_world_generation, ownership_generation=?ownership_state.borrow().generation, movement_epoch=?ownership_state.borrow().movement_epoch, reason="matched pending correction", "accepted validated movement correction acknowledgement");
                    } else {
                        tracing::warn!(account=%account_name, source="server_correction", sequence=movement_mirror_sequence, opcode=frame.opcode, session=movement_session_id, world_generation=movement_world_generation, ownership_generation=?ownership_state.borrow().generation, movement_epoch=?ownership_state.borrow().movement_epoch, reason="no matching pending correction", "dropped unmatched or malformed movement correction acknowledgement");
                        continue;
                    }
                }
                if is_player_movement_opcode(frame.opcode) {
                    let packet_sequence = next_movement_sequence(&mut movement_mirror_sequence);
                    let now = std::time::Instant::now();
                    let decoded = decode_simple_movement(u32::from(frame.opcode), &frame.body);
                    let bot_visual_age = last_bot_visual.map(|(_, _, _, at)| at.elapsed());
                    let bot_locomotion_recent = ownership_state
                        .borrow()
                        .permits(crate::ownership::ClientKind::Bot)
                        || bot_visual_age.is_some_and(|age| age < BOT_VISUAL_ECHO_WINDOW);
                    let matching_bot_echo = match (last_bot_visual, decoded, bot_visual_age) {
                        (Some((visual, visual_mover, bot_opcode, _)), Some((client_mover, _, _, point, orientation)), Some(age)) if age < BOT_VISUAL_ECHO_WINDOW => {
                            movement_matches_bot_visual(visual, visual_mover, bot_opcode, client_mover, point, orientation, frame.opcode, age)
                        }
                        _ => false,
                    };
                    let mut invalid_player_position = false;
                    if let Some((_, _, _, point, _)) = decoded
                        && !matching_bot_echo
                        && let Err(reason) = validate_movement_position(
                            canonical_position,
                            point,
                            100,
                            MovementSource::Player,
                        )
                    {
                        invalid_player_position = true;
                        suppress_client_movement_upstream = true;
                        tracing::warn!(account=%account_name, source="player", opcode=frame.opcode, reason, "dropped player movement outside the accepted position bounds");
                    }
                    let mut explicit_player_intent = decoded.is_some_and(|(guid, flags, _, _, _)| {
                        match validate_player_takeover(frame.opcode, guid, flags, player_guid) {
                            Ok(()) => true,
                            Err(reason) if is_explicit_player_movement_intent(frame.opcode) => {
                                tracing::warn!(account=%account_name, source="player", sequence=movement_mirror_sequence, session=movement_session_id, world_generation=movement_world_generation, opcode=frame.opcode, mover=?guid, expected_mover=?player_guid, flags, owner=?ownership_state.borrow().mode, ownership_generation=?ownership_state.borrow().generation, movement_epoch=?ownership_state.borrow().movement_epoch, reason, "dropped invalid player movement takeover packet");
                                false
                            }
                            Err(_) => false,
                        }
                    });
                    explicit_player_intent &= !invalid_player_position;
                    let invalid_explicit_packet = is_explicit_player_movement_intent(frame.opcode)
                        && !explicit_player_intent
                        && !matching_bot_echo;

                    if invalid_explicit_packet {
                        suppress_client_movement_upstream = true;
                    } else if should_suppress_bot_echo(matching_bot_echo) {
                        suppress_client_movement_upstream = true;
                        tracing::trace!(account=%account_name, source="bot", opcode=frame.opcode, sequence=movement_mirror_sequence, owner=?ownership_state.borrow().mode, session=movement_session_id, world_generation=movement_world_generation, movement_epoch=?ownership_state.borrow().movement_epoch, ownership_generation=?ownership_state.borrow().generation, reason="matched recent bot visual", "suppressed matching bot-authored movement/facing echo");
                    } else if explicit_player_intent {
                        // Movement that does not match the bot's recent visual is player intent.
                        tracing::info!(account=%account_name, source="player", opcode=frame.opcode, sequence=movement_mirror_sequence, owner=?ownership_state.borrow().mode, session=movement_session_id, world_generation=movement_world_generation, movement_epoch=?ownership_state.borrow().movement_epoch, ownership_generation=?ownership_state.borrow().generation, reason="validated explicit player intent", "accepted player movement takeover");
                        last_bot_visual = None;
                        invalidate_movement_mirror(&mut movement_mirror_epoch, &movement_mirror_tx);
                        let (committed, receiver) = oneshot::channel();
                        if account.session_tx.send(SessionMessage::PlayerMovement { at: now, committed }).await.is_err() {
                            tracing::warn!(account=%account_name, opcode=frame.opcode, reason="session actor unavailable", "dropped player movement because ownership handoff could not be committed");
                            continue;
                        }
                        if receiver.await.is_err() {
                            tracing::warn!(account=%account_name, opcode=frame.opcode, reason="ownership handoff did not commit", "dropped player movement because ownership handoff did not complete");
                            continue;
                        }
                    } else if bot_locomotion_recent {
                        // AzerothCore intentionally does not echo the mover's movement
                        // packet back to that same player. We mirror bot movement locally
                        // for the attended client, which can cause the stock client to emit
                        // passive heartbeat/stop feedback. That feedback is not new human
                        // intent and must not fence the bot or overwrite its upstream path.
                        suppress_client_movement_upstream = true;
                        tracing::trace!(account=%account_name, source="player", opcode=frame.opcode, sequence=movement_mirror_sequence, owner=?ownership_state.borrow().mode, session=movement_session_id, world_generation=movement_world_generation, movement_epoch=?ownership_state.borrow().movement_epoch, ownership_generation=?ownership_state.borrow().generation, reason="passive feedback while bot locomotion is active", "dropped passive client movement feedback");
                    }

                    if decoded.is_none() {
                        suppress_client_movement_upstream = true;
                        let owner = *ownership_state.borrow();
                        tracing::warn!(account=%account_name, source="player", sequence=packet_sequence, session=movement_session_id, opcode=frame.opcode, owner=?owner.mode, attended_control=?owner.attended_control, ownership_generation=owner.generation.get(), movement_epoch=owner.movement_epoch.get(), world_generation=movement_world_generation, reason="malformed movement packet layout", "dropped malformed player movement packet");
                    } else if decoded.is_some_and(|(guid, _, _, _, _)| player_guid != Some(guid))
                        && !matching_bot_echo
                        && !invalid_explicit_packet
                    {
                        suppress_client_movement_upstream = true;
                        let owner = *ownership_state.borrow();
                        tracing::warn!(account=%account_name, source="player", sequence=packet_sequence, session=movement_session_id, opcode=frame.opcode, mover=?decoded.map(|value| value.0), expected_mover=?player_guid, owner=?owner.mode, attended_control=?owner.attended_control, ownership_generation=owner.generation.get(), movement_epoch=owner.movement_epoch.get(), world_generation=movement_world_generation, reason="mover GUID is not the active player", "dropped player movement packet");
                    }

                    if !suppress_client_movement_upstream && decoded.is_some() {
                        player_movement_stamp = Some(MovementStamp::new(
                            MovementVisualSource::Player,
                            movement_session_id,
                            packet_sequence,
                            *ownership_state.borrow(),
                            movement_mirror_epoch,
                            movement_world_generation,
                        ));
                    }

                    if let Some((guid, flags, client_time, point, orientation)) = decoded
                        && (player_guid.is_none() || player_guid == Some(guid))
                        && let Some(mut current) = canonical_position
                    {
                        current.point = point;
                        current.orientation = orientation;
                        let source = if explicit_player_intent {
                            "client_intent"
                        } else if matching_bot_echo {
                            "client_bot_echo"
                        } else if suppress_client_movement_upstream {
                            "client_movement_suppressed"
                        } else {
                            "client_movement"
                        };
                        shared.action_logs.position(
                            &account_name,
                            source,
                            current,
                            Some(guid),
                            Some(u32::from(frame.opcode)),
                            Some(flags),
                            Some(client_time),
                        ).await;
                        if !suppress_client_movement_upstream {
                            canonical_position = Some(current);
                            canonical_flags = flags;
                            movement_clock.observe(client_time);
                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::PlayerPosition { position: current, moving: flags != 0, flags, client_time })).await;
                        }
                    }
                }
                if frame.opcode == 0x0095 {
                    if *assistance_armed.borrow() && is_afk_chat_request(&frame) {
                        tracing::info!(account=%account_name, "suppressed AFK request while bot assistance is enabled");
                        continue;
                    }
                    if let Some((family, text)) = proxy_chat(&frame.body) {
                        if crate::commands::chat::is_local_namespace(text) {
                            tracing::info!(account=%account_name, ?family, command=%text, "local proxy chat command received");
                        }
                        let mission_id = MissionId(account.mission_counter.fetch_add(1, Ordering::Relaxed));
                        match crate::commands::parse_local(text, family, true, mission_id) {
                            Ok(Some(crate::commands::LocalCommand::Bot(command))) => {
                                match execute_bot_command(&account, command).await {
                                    Ok(lines) => {
                                        for line in lines {
                                            let notice = format!("[wow-bot] {line}");
                                            let _ = write_bot_notice(&mut dw, &mut down_enc, &notice).await;
                                        }
                                    }
                                    Err(error) => {
                                        tracing::warn!(account=%account_name, %error, "local bot command failed");
                                        let notice = format!("[wow-bot] {error}");
                                        let _ = write_bot_notice(&mut dw, &mut down_enc, &notice).await;
                                    }
                                }
                                continue;
                            }
                            Ok(Some(crate::commands::LocalCommand::Log(log))) => {
                                let notice =
                                    handle_log_command(&shared.action_logs, &account_name, log).await;
                                let _ = write_bot_notice(&mut dw, &mut down_enc, &notice).await;
                                continue;
                            }
                            Ok(None) => {}
                            Err(error) => { tracing::warn!(account=%account_name, %error, command=%text, "invalid local proxy command"); continue; }
                        }
                    }
                }
                if suppress_client_movement_upstream {
                    continue;
                }
                log_loot_wire_packet(&account_name, "client_to_server", u32::from(frame.opcode), &frame.body);
                if frame.opcode == 0x015D && frame.body.len() >= 8 {
                    let guid = u64::from_le_bytes(frame.body[0..8].try_into().unwrap());
                    if bot_loot_target.is_some_and(|target| target.0 == guid) {
                        tracing::info!(account=%account_name, loot_guid=guid, "player loot request superseded pending bot loot ownership");
                        bot_loot_target = None;
                    }
                    let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootOpened { target: EntityId(guid), ownership: wow_state::observation::LootOwnership::Player })).await;
                }
                if frame.opcode == 0x015F {
                    let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootClosed { ownership: wow_state::observation::LootOwnership::Player })).await;
                }
                if frame.opcode == CMSG_WARDEN_DATA::OPCODE { warden.client_to_server(&mut frame.body); }
                if let Some(stamp) = player_movement_stamp.or(proxy_handoff_stamp) {
                    let movement_guard = account.movement_gate.clone().lock_owned().await;
                    let owner = *ownership_state.borrow();
                    let source_is_permitted = match stamp.source {
                        MovementVisualSource::Player => owner.permits(crate::ownership::ClientKind::Player),
                        MovementVisualSource::ProxyHandoff => !owner.transitioning(),
                        MovementVisualSource::Bot | MovementVisualSource::Server => false,
                    };
                    if !source_is_permitted
                        || !movement_visual_is_current(
                            stamp,
                            stamp.source,
                            movement_session_id,
                            owner.generation.get(),
                            owner.movement_epoch.get(),
                            movement_mirror_epoch,
                            movement_world_generation,
                            last_player_sequence,
                        )
                    {
                        tracing::warn!(account=%account_name, source=?stamp.source, sequence=stamp.sequence, session=stamp.session_id, owner=?owner.mode, ownership_generation=stamp.ownership_generation, movement_epoch=stamp.movement_epoch, world_generation=stamp.world_generation, reason="movement fence changed before upstream write", "dropped stale player movement packet");
                        continue;
                    }
                    if stamp.source == MovementVisualSource::Player {
                        last_player_sequence = stamp.sequence;
                    }
                    let write_result = write_client_frame(&mut uw, &mut up_enc, &frame).await;
                    drop(movement_guard);
                    if let Err(error) = write_result { break Err(error); }
                } else if let Err(e)=write_client_frame(&mut uw, &mut up_enc, &frame).await { break Err(e); }
            }
            command = commands.recv() => {
                match command {
                    Ok(dispatch) => {
                        let owner = *ownership_state.borrow();
                        if !dispatch.is_current(owner, movement_world_generation)
                            || !dispatch.is_new_sequence(&mut last_bot_command_sequence)
                        {
                            let (generation, movement_epoch, sequence, world_generation) = match &dispatch {
                                crate::configured_session::GameplayDispatch::BotMovement { generation, movement_epoch, sequence, world_generation, .. } =>
                                    (Some(generation.get()), Some(movement_epoch.get()), Some(*sequence), Some(*world_generation)),
                                crate::configured_session::GameplayDispatch::Unfenced(_) => (None, None, None, None),
                            };
                            tracing::warn!(account=%account_name, source="bot", sequence, session=movement_session_id, ownership_generation=generation, current_generation=owner.generation.get(), movement_epoch, current_epoch=owner.movement_epoch.get(), world_generation, current_world_generation=movement_world_generation, owner=?owner.mode, attended_control=?owner.attended_control, reason="stale stamp or sequence", "dropped stale bot movement command before upstream routing");
                            continue;
                        }
                        let command = dispatch.command().clone();
                        {
                        if let Some(target) = bot_loot_target_for_command(&command) {
                            bot_loot_target = Some(target);
                            bot_loot_response_seen = false;
                            tracing::info!(account=%account.config.account_name, loot_guid=target.0, ?command, "bot loot command queued for transmission");
                        }
                        let bot_cast = bot_targeted_cast(&command);
                        if let Some((spell, target)) = bot_cast {
                            last_bot_cast = Some((spell, target, std::time::Instant::now()));
                        }
                        let gameobject_report_use = gameobject_report_use_for_command(&command);
                        let publish_movement_prediction =
                            should_publish_movement_prediction(&command);
                        let movement_context = gameplay_movement_context(
                            controlled_mover,
                            controlled_position,
                            controlled_flags,
                            player_guid,
                            canonical_position,
                            canonical_flags,
                        );
                        let bot_movement_elapsed_ms = last_bot_movement_at
                            .map(|at| at.elapsed().as_millis().min(u128::from(u32::MAX)) as u32)
                            .unwrap_or_default();
                        if let GameplayCommand::MoveTo(destination) = &command
                            && let Err(reason) = validate_movement_position(movement_context.position, *destination, bot_movement_elapsed_ms, MovementSource::Bot)
                        {
                            tracing::warn!(account=%account_name, source="bot", destination=?destination, reason, "dropped bot movement outside the accepted position bounds");
                            continue;
                        }
                        let command_is_stop = matches!(&command, GameplayCommand::StopMovement);
                        let command_is_move = matches!(&command, GameplayCommand::MoveTo(_));
                        match encode_gameplay_command(
                            command,
                            movement_context.mover,
                            movement_context.position,
                            movement_context.flags,
                            &mut movement_clock,
                        ) {
                            Ok(Some((frame, movement))) => {
                                tracing::info!(account=%account_name, opcode=frame.opcode, "bot gameplay packet transmitted");
                                shared.action_logs.packet(
                                    &account_name,
                                    "bot",
                                    "C2S",
                                    frame.opcode,
                                    &frame.body,
                                ).await;
                                log_loot_wire_packet(&account_name, "bot_to_server", frame.opcode, &frame.body);
                                let movement_guard = if dispatch.bot_movement_stamp().is_some() {
                                    Some(account.movement_gate.clone().lock_owned().await)
                                } else {
                                    None
                                };
                                if !dispatch.is_current(*ownership_state.borrow(), movement_world_generation) {
                                    tracing::warn!(account=%account_name, opcode=frame.opcode, "dropped bot movement command after ownership changed during encoding");
                                    continue;
                                }
                                if let Err(e)=write_client_frame(&mut uw, &mut up_enc, &frame).await { break Err(e); }
                                if command_is_move { last_bot_movement_at = Some(std::time::Instant::now()); }
                                if let Some((spell, target)) = bot_cast {
                                    tracing::info!(account=%account_name, opcode=frame.opcode, spell, ?target, "bot cast request transmitted");
                                }
                                if let Some((target, report)) = gameobject_report_use {
                                    tracing::info!(account=%account_name, opcode=report.opcode, ?target, "bot game-object report-use packet transmitted");
                                    shared.action_logs.packet(
                                        &account_name,
                                        "bot",
                                        "C2S",
                                        report.opcode,
                                        &report.body,
                                    ).await;
                                    if let Err(e)=write_client_frame(&mut uw, &mut up_enc, &report).await { break Err(e); }
                                }
                                if let Some((position, moving, flags, client_time)) = movement {
                                    // A set-facing packet is only a request. Keep the last
                                    // server-observed orientation until the upstream object
                                    // update confirms the turn.
                                    let current_owner = *ownership_state.borrow();
                                    let bot_visual_stamp = dispatch.bot_movement_stamp();
                                    if publish_movement_prediction
                                        && (bot_visual_stamp.is_none() || dispatch.is_current(current_owner, movement_world_generation))
                                    {
                                        shared.action_logs.position(
                                            &account_name,
                                            "bot_prediction",
                                            position,
                                            movement_context.mover,
                                            Some(u32::from(frame.opcode)),
                                            Some(flags),
                                            Some(client_time),
                                        ).await;
                                        if controlled_mover.is_some() {
                                            controlled_position = Some(position);
                                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::ControlledMover { mover: controlled_mover, position: Some(position), flags })).await;
                                        } else {
                                            canonical_position = Some(position);
                                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::PlayerPosition { position, moving, flags, client_time })).await;
                                        }
                                    }
                                    let visual_source = if let Some((_, _, _, world_generation)) = bot_visual_stamp
                                        && dispatch.is_current(*ownership_state.borrow(), movement_world_generation)
                                    {
                                        Some((MovementVisualSource::Bot, world_generation))
                                    } else if command_is_stop {
                                        Some((MovementVisualSource::ProxyHandoff, movement_world_generation))
                                    } else {
                                        None
                                    };
                                    if let Some((source, stamped_world_generation)) = visual_source {
                                        if movement_visual_source_is_owned(source, *ownership_state.borrow()) {
                                            if source == MovementVisualSource::Bot {
                                                last_bot_visual = Some((position, movement_context.mover.unwrap_or(EntityId(0)), frame.opcode, std::time::Instant::now()));
                                            }
                                            let mover = movement_context.mover.unwrap_or(EntityId(0));
                                            if projected_mover != Some(mover) {
                                                if let Some(start) = movement_context.position {
                                                    server_motion.reset_to(mover, start, client_time.wrapping_sub(100));
                                                }
                                                projected_mover = Some(mover);
                                            }
                                            let visual = if command_is_stop {
                                                server_motion.stop(mover, position)
                                            } else {
                                                server_motion.project(mover, position, client_time, command_is_move)
                                            };
                                            if let Some(visual) = visual {
                                                let sequence = next_movement_sequence(&mut movement_mirror_sequence);
                                                let stamp = MovementStamp::new(
                                                    source,
                                                    movement_session_id,
                                                    sequence,
                                                    *ownership_state.borrow(),
                                                    movement_mirror_epoch,
                                                    stamped_world_generation,
                                                );
                                                movement_mirror_tx.send_replace(Some(MovementMirror {
                                                    stamp,
                                                    frame: visual,
                                                }));
                                                tracing::debug!(account=%account_name, source=?stamp.source, sequence=stamp.sequence, session=stamp.session_id, ownership_generation=stamp.ownership_generation, movement_epoch=stamp.movement_epoch, world_generation=stamp.world_generation, reason="accepted", "stamped movement visual");
                                            }
                                        }
                                    }
                                }
                                drop(movement_guard);
                            }
                            Ok(None) => {}
                            Err(other) => tracing::warn!(account=%account_name, command=%other, "gameplay command has no WotLK encoder yet; command rejected before upstream write"),
                        }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!(account=%account_name, skipped=n, "gameplay command bus lagged"),
                    Err(broadcast::error::RecvError::Closed) => {}
                }
            }
            mirror = movement_mirror_rx.changed() => {
                match mirror {
                    Ok(()) => {
                        let update = movement_mirror_rx.borrow_and_update().clone();
                        if let Some(update) = update {
                            let current_owner = *ownership_state.borrow();
                            let source_owned = movement_visual_source_is_owned(update.stamp.source, current_owner);
                            if !source_owned
                                || !movement_visual_is_current(update.stamp, update.stamp.source, movement_session_id, current_owner.generation.get(), current_owner.movement_epoch.get(), movement_mirror_epoch, movement_world_generation, last_mirrored_sequence) {
                                last_mirrored_sequence = last_mirrored_sequence.max(update.stamp.sequence);
                                tracing::debug!(account=%account_name, source=?update.stamp.source, sequence=update.stamp.sequence, session=update.stamp.session_id, current_session=movement_session_id, update_generation=update.stamp.ownership_generation, current_generation=?current_owner.generation, update_epoch=update.stamp.movement_epoch, current_epoch=?current_owner.movement_epoch, world_generation=update.stamp.world_generation, reason="stale or mismatched movement visual", "dropped movement mirror");
                                continue;
                            }
                            if update.stamp.sequence > last_mirrored_sequence.saturating_add(1) {
                                tracing::warn!(account=%account_name, skipped=update.stamp.sequence - last_mirrored_sequence - 1, sequence=update.stamp.sequence, "movement mirror fell behind; sending the latest bot position");
                            }
                            shared.action_logs.packet(
                                &account_name,
                                "bot_mirror",
                                "S2C",
                                u32::from(update.frame.opcode),
                                &update.frame.body,
                            ).await;
                            let movement_guard = account.movement_gate.clone().lock_owned().await;
                            let owner_before_write = *ownership_state.borrow();
                            let source_owned = movement_visual_source_is_owned(update.stamp.source, owner_before_write);
                            if !source_owned
                                || !movement_visual_is_current(
                                    update.stamp,
                                    update.stamp.source,
                                    movement_session_id,
                                    owner_before_write.generation.get(),
                                    owner_before_write.movement_epoch.get(),
                                    movement_mirror_epoch,
                                    movement_world_generation,
                                    last_mirrored_sequence,
                                )
                            {
                                last_mirrored_sequence = last_mirrored_sequence.max(update.stamp.sequence);
                                tracing::debug!(account=%account_name, source=?update.stamp.source, sequence=update.stamp.sequence, session=update.stamp.session_id, ownership_generation=update.stamp.ownership_generation, movement_epoch=update.stamp.movement_epoch, world_generation=update.stamp.world_generation, reason="fence changed before downstream write", "dropped queued movement visual");
                                continue;
                            }
                            if let Err(error) = write_server_frame(&mut dw, &mut down_enc, &update.frame)
                                .await
                                .with_context(|| format!("failed to mirror bot movement update {} to the player client", update.stamp.sequence))
                            {
                                tracing::error!(account=%account_name, sequence=update.stamp.sequence, %error, "player movement mirror failed; closing this world bridge for safe reconnect");
                                break Err(error);
                            }
                            last_mirrored_sequence = update.stamp.sequence;
                            drop(movement_guard);
                        }
                    }
                    Err(_) => {
                        break Err(anyhow::anyhow!("bot movement mirror channel closed while the world bridge was active"));
                    }
                }
            }
            fence = visual_fence.recv() => {
                match fence {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        last_bot_visual = None;
                        invalidate_movement_mirror(&mut movement_mirror_epoch, &movement_mirror_tx);
                    }
                    Err(broadcast::error::RecvError::Closed) => {}
                }
            }
            server = read_server_frame(&mut ur, &mut up_dec) => {
                let mut frame = match server { Ok(v)=>v, Err(e)=>break Err(e) };
                if matches!(u32::from(frame.opcode), 0x003E | 0x0236) {
                    if !fence_world_movement(
                        &account,
                        &mut movement_world_generation,
                        &mut movement_mirror_epoch,
                        &movement_mirror_tx,
                    ).await {
                        break Err(anyhow::anyhow!("world-transition movement fence failed"));
                    }
                    last_bot_visual = None;
                    pending_near_teleport_ack = None;
                    pending_near_teleport_position = None;
                    pending_movement_corrections.clear();
                }
                if u32::from(frame.opcode) == 0x003E {
                    match player_guid.and_then(|guid| {
                        new_world(&frame.body, guid.0).map(|observation| (guid, observation))
                    }) {
                        Some((guid, observation)) => {
                            let ProtocolObservation::WorldChanged { position, .. } = &observation else {
                                unreachable!("new-world parser returns a world-change observation");
                            };
                            let position = *position;
                            object_observer.change_world(position, Some(guid))?;
                            canonical_position = Some(position);
                            server_motion.reset_to(guid, position, movement_clock.current_timestamp());
                            projected_mover = Some(guid);
                            canonical_flags = 0;
                            controlled_mover = None;
                            controlled_position = None;
                            controlled_flags = 0;
                            let _ = account
                                .session_tx
                                .send(SessionMessage::WorldAuthoritative(false))
                                .await;
                            let _ = account
                                .session_tx
                                .send(SessionMessage::Observation(observation))
                                .await;
                            let _ = account
                                .session_tx
                                .send(SessionMessage::WorldAuthoritative(true))
                                .await;
                            tracing::info!(account=%account_name, map=position.map, "attended world transfer applied and new-world observation restored");
                        }
                        None => {
                            tracing::error!(account=%account_name, body_len=frame.body.len(), has_player_guid=player_guid.is_some(), "failed to parse attended SMSG_NEW_WORLD; world state remains unchanged");
                        }
                    }
                }
                let mut server_movement_stamp = if is_server_movement_visual_opcode(u32::from(frame.opcode)) {
                    let sequence = next_movement_sequence(&mut movement_mirror_sequence);
                    Some(MovementStamp::new(
                        MovementVisualSource::Server,
                        movement_session_id,
                        sequence,
                        *ownership_state.borrow(),
                        movement_mirror_epoch,
                        movement_world_generation,
                    ))
                } else {
                    None
                };
                if u32::from(frame.opcode) == 0x00C7 {
                    let parsed = parse_server_near_teleport(&frame.body)
                        .filter(|(mover, _, _, _)| player_guid == Some(*mover) || controlled_mover == Some(*mover));
                    pending_near_teleport_ack = parsed.map(|(mover, flags, _, _)| (mover, flags));
                    pending_near_teleport_position = parsed.map(|(mover, _, point, orientation)| (
                        mover,
                        WorldPosition { map: canonical_position.map(|position| position.map).unwrap_or_default(), point, orientation },
                    ));
                    if pending_near_teleport_ack.is_none() {
                        tracing::warn!(account=%account_name, opcode=frame.opcode, "server near-teleport request has an invalid or unexpected mover; client acknowledgement will be rejected");
                    }
                } else if let Some(expected) = movement_correction_expectation(u32::from(frame.opcode), &frame.body)
                    && (player_guid == Some(expected.mover) || controlled_mover == Some(expected.mover))
                {
                    if pending_movement_corrections.len() == 64 {
                        pending_movement_corrections.pop_front();
                    }
                    pending_movement_corrections.push_back(expected);
                }
                shared.action_logs.packet(
                    &account_name,
                    "world",
                    "S2C",
                    u32::from(frame.opcode),
                    &frame.body,
                ).await;
                log_loot_wire_packet(&account_name, "server_to_bot", u32::from(frame.opcode), &frame.body);
                log_bot_cast_progress(
                    &account_name,
                    u32::from(frame.opcode),
                    &frame.body,
                    player_guid,
                    &last_bot_cast,
                );
                const SMSG_LOOT_RESPONSE_OPCODE: u32 = 0x0160;
                const SMSG_LOOT_RELEASE_RESPONSE_OPCODE: u32 = 0x0161;
                const SMSG_LOOT_REMOVED_OPCODE: u32 = 0x0162;
                const SMSG_LOOT_CLEAR_MONEY_OPCODE: u32 = 0x0163;
                const CMSG_AUTOSTORE_LOOT_ITEM_OPCODE: u32 = 0x0108;
                const CMSG_LOOT_MONEY_OPCODE: u32 = 0x015E;
                const CMSG_LOOT_RELEASE_OPCODE: u32 = 0x015F;
                let mut suppress_bot_loot_frame = false;
                if u32::from(frame.opcode) == SMSG_LOOT_RESPONSE_OPCODE {
                    tracing::info!(account=%account_name, opcode=u32::from(frame.opcode), body_len=frame.body.len(), packet_guid=?frame.body.get(..8).and_then(|bytes| <[u8; 8]>::try_from(bytes).ok()).map(u64::from_le_bytes), expected_guid=?bot_loot_target, "loot response packet received");
                    if let Some(loot) = parse_loot_response(&frame.body) {
                        if bot_loot_target.is_some_and(|target| target.0 == loot.guid) {
                            bot_loot_response_seen = true;
                            suppress_bot_loot_frame = true;
                            if loot.loot_type == 0 {
                                let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootRejected { target: EntityId(loot.guid), loot_type: loot.loot_type, error: loot.error })).await;
                                tracing::warn!(account=%account_name, loot_guid=loot.guid, loot_type=loot.loot_type, loot_error=?loot.error, "server rejected bot-owned loot request");
                                let release = ClientFrame { opcode: CMSG_LOOT_RELEASE_OPCODE, body: loot.guid.to_le_bytes().to_vec() };
                                log_loot_wire_packet(&account_name, "bot_to_server", release.opcode, &release.body);
                                shared.action_logs.packet(&account_name, "proxy", "C2S", release.opcode, &release.body).await;
                                if let Err(error) = write_client_frame(&mut uw, &mut up_enc, &release).await { break Err(error); }
                            } else {
                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootOpened { target: EntityId(loot.guid), ownership: wow_state::observation::LootOwnership::Bot })).await;
                            tracing::info!(account=%account_name, loot_guid=loot.guid, loot_type=loot.loot_type, gold=loot.gold, slots=?loot.slots, "bot-owned loot window opened; collecting slots");
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            if loot.gold > 0 {
                                let money = ClientFrame { opcode: CMSG_LOOT_MONEY_OPCODE, body: Vec::new() };
                                log_loot_wire_packet(&account_name, "bot_to_server", money.opcode, &money.body);
                                shared.action_logs.packet(&account_name, "proxy", "C2S", money.opcode, &money.body).await;
                                if let Err(e) = write_client_frame(&mut uw, &mut up_enc, &money).await { break Err(e); }
                            }
                            let mut loot_slot_write_error = None;
                            for slot in loot.slots {
                                let take = ClientFrame { opcode: CMSG_AUTOSTORE_LOOT_ITEM_OPCODE, body: vec![slot] };
                                log_loot_wire_packet(&account_name, "bot_to_server", take.opcode, &take.body);
                                shared.action_logs.packet(&account_name, "proxy", "C2S", take.opcode, &take.body).await;
                                if let Err(error) = write_client_frame(&mut uw, &mut up_enc, &take).await {
                                    loot_slot_write_error = Some(error);
                                    break;
                                }
                            }
                            if let Some(error) = loot_slot_write_error {
                                break Err(error);
                            }
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            let release = ClientFrame { opcode: CMSG_LOOT_RELEASE_OPCODE, body: loot.guid.to_le_bytes().to_vec() };
                            log_loot_wire_packet(&account_name, "bot_to_server", release.opcode, &release.body);
                            shared.action_logs.packet(&account_name, "proxy", "C2S", release.opcode, &release.body).await;
                            if let Err(e) = write_client_frame(&mut uw, &mut up_enc, &release).await { break Err(e); }
                            }
                        }
                        else {
                            tracing::warn!(account=%account_name, received_guid=loot.guid, expected_guid=?bot_loot_target, loot_type=loot.loot_type, "loot response did not match active bot loot target");
                        }
                    } else {
                        tracing::warn!(account=%account_name, body_len=frame.body.len(), "failed to parse loot response packet");
                    }
                } else if matches!(u32::from(frame.opcode), SMSG_LOOT_RELEASE_RESPONSE_OPCODE | SMSG_LOOT_REMOVED_OPCODE | SMSG_LOOT_CLEAR_MONEY_OPCODE) && bot_loot_target.is_some() {
                    suppress_bot_loot_frame = true;
                    if u32::from(frame.opcode) == SMSG_LOOT_RELEASE_RESPONSE_OPCODE {
                        tracing::info!(account=%account_name, body_len=frame.body.len(), packet_guid=?frame.body.get(..8).and_then(|bytes| <[u8; 8]>::try_from(bytes).ok()).map(u64::from_le_bytes), expected_guid=?bot_loot_target, "loot release response received");
                        if let Some(target) = rejected_loot_target_without_response(
                            bot_loot_target,
                            bot_loot_response_seen,
                        )
                        {
                            tracing::warn!(account=%account_name, loot_guid=target.0, "server released bot loot request without a loot response");
                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootRejected {
                                target,
                                loot_type: 0,
                                error: None,
                            })).await;
                        }
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootClosed { ownership: wow_state::observation::LootOwnership::Bot })).await;
                        tracing::info!(account=%account_name, "bot-owned loot transaction released");
                        bot_loot_target = None;
                        bot_loot_response_seen = false;
                    }
                }
                const SMSG_CAST_FAILED_OPCODE: u32 = 0x0130;
                if u32::from(frame.opcode) == SMSG_CAST_FAILED_OPCODE {
                    if let Some((cast_count, spell, reason, target)) =
                        take_bot_cast_failure(&frame.body, &mut last_bot_cast)
                    {
                        tracing::info!(account=%account_name, cast_count, spell, reason, ?target, "authoritative bot cast failure observed");
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::CastFailed { spell, reason, target })).await;
                    }
                }
                const SMSG_LOGIN_VERIFY_WORLD_OPCODE: u32 = 0x0236;
                if u32::from(frame.opcode) == SMSG_LOGIN_VERIFY_WORLD_OPCODE {
                    tracing::info!(account=%account_name, body_len=frame.body.len(), has_player_guid=player_guid.is_some(), "received SMSG_LOGIN_VERIFY_WORLD");
                    match player_guid {
                        Some(guid) => match login_verify_world(&frame.body, guid.0) {
                            Some(observation) => {
                                if let ProtocolObservation::EnteredWorld { position: Some(position), .. } = &observation {
                                    object_observer.set_world(*position, Some(guid))?;
                                    canonical_position = Some(*position);
                                    server_motion.reset_to(guid, *position, movement_clock.current_timestamp());
                                    projected_mover = Some(guid);
                                    canonical_flags = 0;
                                    let _ = account.command_bus.send(crate::configured_session::GameplayDispatch::Unfenced(GameplayCommand::StopMovement));
                                }
                                tracing::info!(account=%account_name, ?guid, "authoritative world-entry observation received");
                                let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                                let _ = account.session_tx.send(SessionMessage::WorldAuthoritative(true)).await;
                            }
                            None => {
                                tracing::error!(account=%account_name, body_len=frame.body.len(), "failed to parse SMSG_LOGIN_VERIFY_WORLD; world authority remains false");
                            }
                        },
                        None => {
                            tracing::error!(account=%account_name, body_len=frame.body.len(), "SMSG_LOGIN_VERIFY_WORLD arrived before character GUID was captured; world authority remains false");
                        }
                    }
                }
                forward_protocol_observations(&account, &account_name, u32::from(frame.opcode), &frame.body).await;
                match object_observer.observe(frame.opcode, &frame.body).await {
                    Ok(observations) => {
                        if server_movement_stamp.is_none()
                            && observations.iter().any(is_movement_observation)
                        {
                            let sequence = next_movement_sequence(&mut movement_mirror_sequence);
                            server_movement_stamp = Some(MovementStamp::new(
                                MovementVisualSource::Server,
                                movement_session_id,
                                sequence,
                                *ownership_state.borrow(),
                                movement_mirror_epoch,
                                movement_world_generation,
                            ));
                        }
                        for observation in observations {
                            match &observation {
                            ProtocolObservation::PlayerPosition { position, flags, .. } => {
                                canonical_position = Some(*position);
                                canonical_flags = *flags;
                                shared.action_logs.position(
                                    &account_name,
                                    "server_player_position",
                                    *position,
                                    player_guid,
                                    Some(u32::from(frame.opcode)),
                                    Some(*flags),
                                    None,
                                ).await;
                            }
                            ProtocolObservation::ControlledMover { mover, position, flags } => {
                                controlled_mover = *mover;
                                controlled_position = *position;
                                controlled_flags = *flags;
                                if let Some(position) = position {
                                    shared.action_logs.position(
                                        &account_name,
                                        "server_controlled_mover",
                                        *position,
                                        *mover,
                                        Some(u32::from(frame.opcode)),
                                        Some(*flags),
                                        None,
                                    ).await;
                                }
                                tracing::info!(account=%account_name, ?controlled_mover, has_position=controlled_position.is_some(), "authoritative controlled mover changed");
                            }
                            _ => {}
                            }
                            let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                        }
                    }
                    Err(error) => tracing::warn!(account=%account_name, opcode=frame.opcode, %error, "Tentacli object observation failed; packet still relayed to client"),
                }
                if u32::from(frame.opcode) == SMSG_WARDEN_DATA::OPCODE { warden.server_to_client(&mut frame.body); }
                if !suppress_bot_loot_frame {
                    if let Some(stamp) = server_movement_stamp {
                        let movement_guard = account.movement_gate.clone().lock_owned().await;
                        let owner = *ownership_state.borrow();
                        if owner.transitioning()
                            || !movement_visual_is_current(
                                stamp,
                                MovementVisualSource::Server,
                                movement_session_id,
                                owner.generation.get(),
                                owner.movement_epoch.get(),
                                movement_mirror_epoch,
                                movement_world_generation,
                                last_server_sequence,
                            )
                        {
                            tracing::warn!(account=%account_name, source=?stamp.source, sequence=stamp.sequence, session=stamp.session_id, owner=?owner.mode, ownership_generation=stamp.ownership_generation, movement_epoch=stamp.movement_epoch, world_generation=stamp.world_generation, reason="movement fence changed before downstream write", "dropped stale server movement visual");
                            continue;
                        }
                        last_server_sequence = stamp.sequence;
                        let write_result = write_server_frame(&mut dw, &mut down_enc, &frame).await;
                        drop(movement_guard);
                        if let Err(error) = write_result { break Err(error); }
                    } else if let Err(e)=write_server_frame(&mut dw, &mut down_enc, &frame).await { break Err(e); }
                }
            }
        }
    };
    account
        .session_tx
        .send(SessionMessage::PlayerDetached { connection })
        .await
        .ok();
    result
}

async fn wait_for_headless_state(
    account: &AccountRuntime,
    expected: bool,
    timeout: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if account.headless_active.load(Ordering::Acquire) == expected {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("headless session did not reach active={expected} before handoff deadline");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn headless_session_manager(shared: SharedRuntime, account: AccountRuntime) {
    let mut enabled = account.headless_enabled.subscribe();
    let mut retry = Duration::from_secs(1);
    loop {
        while !*enabled.borrow_and_update() {
            if enabled.changed().await.is_err() {
                return;
            }
        }
        tracing::info!(account=%account.config.account_name, ?account.config.character, "headless autonomous session requested");
        account.headless_active.store(true, Ordering::Release);
        let session_started = tokio::time::Instant::now();
        let result =
            run_headless_world_session(shared.clone(), account.clone(), &mut enabled).await;
        let connected_for = session_started.elapsed();
        account.headless_active.store(false, Ordering::Release);
        let _ = account
            .session_tx
            .send(SessionMessage::WorldAuthoritative(false))
            .await;
        let _ = account
            .session_tx
            .send(SessionMessage::UpstreamConnected(false))
            .await;
        if !*enabled.borrow() {
            tracing::info!(account=%account.config.account_name, "headless autonomous session yielded to attended player");
            retry = Duration::from_secs(1);
            continue;
        }
        match result {
            Ok(()) => {
                tracing::info!(account=%account.config.account_name, connected_for_ms=connected_for.as_millis(), retry_after_ms=retry.as_millis(), "headless autonomous session ended; reconnect scheduled")
            }
            Err(error) => {
                if is_missing_warden_client_image(&error) {
                    tracing::error!(
                        account=%account.config.account_name,
                        error=%format_args!("{error:#}"),
                        "headless session stopped; configure proxy.warden_client_image before retrying"
                    );
                    let _ = account.headless_enabled.send(false);
                    retry = Duration::from_secs(1);
                    continue;
                }
                tracing::warn!(account=%account.config.account_name, connected_for_ms=connected_for.as_millis(), retry_after_ms=retry.as_millis(), error=%format_args!("{error:#}"), "headless autonomous session failed; reconnect scheduled")
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(retry) => {},
            changed = enabled.changed() => { if changed.is_err() { return; } }
        }
        retry = (retry * 2).min(Duration::from_secs(15));
    }
}

fn is_missing_warden_client_image(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string() == "Warden checks require proxy.warden_client_image")
}

async fn run_headless_world_session(
    shared: SharedRuntime,
    account: AccountRuntime,
    enabled: &mut watch::Receiver<bool>,
) -> Result<()> {
    if !*enabled.borrow() {
        return Ok(());
    }
    let upstream_login = upstream_login(
        &upstream_config(&shared, &account),
        &account.config.password,
    )
    .await?;
    let addr = advertised(&upstream_login.world_host, upstream_login.world_port);
    let mut upstream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("connect headless upstream world {addr}"))?;
    let challenge = world_expect_server_message::<SMSG_AUTH_CHALLENGE, _>(&mut upstream).await?;
    let normalized = NormalizedString::new(&account.config.account_name.to_uppercase())?;
    let seed = ProofSeed::new();
    let client_seed = seed.seed();
    let (proof, crypto) = seed.into_client_header_crypto(
        &normalized,
        upstream_login.session_key,
        challenge.server_seed,
    );
    CMSG_AUTH_SESSION {
        client_build: 12340,
        login_server_id: 0,
        username: account.config.account_name.to_uppercase(),
        login_server_type: 0,
        client_seed,
        region_id: 0,
        battleground_id: 0,
        realm_id: upstream_login.realm_id,
        dos_response: 0,
        client_proof: proof,
        addon_info: Vec::new(),
    }
    .tokio_write_unencrypted_client(&mut upstream)
    .await?;
    let (mut enc, mut dec) = crypto.split();
    let (mut reader, mut writer) = upstream.into_split();
    let mut warden = WardenClient::new(
        &upstream_login.session_key,
        shared.config.warden_client_image.as_deref(),
    )?;
    let mut commands = account.command_bus.subscribe();
    let mut object_observer =
        ObjectObservationRuntime::new().context("initialize headless Tentacli object observer")?;
    let mut player_guid = None;
    let mut canonical_position = None;
    let mut canonical_flags = 0_u32;
    let mut controlled_mover = None;
    let mut controlled_position = None;
    let mut controlled_flags = 0_u32;
    let mut movement_clock = MovementClock::default();
    let mut bot_loot_target = None;
    let mut bot_loot_response_seen = false;
    let mut last_bot_cast: Option<(u32, Option<EntityId>, std::time::Instant)> = None;
    let mut char_enum_requested = false;
    let mut player_login_requested = false;
    let mut world_transfer_pending = false;
    let mut movement_world_generation = fence_actor_movement(&account)
        .await
        .context("could not commit initial headless movement fence")?
        .1;
    let mut last_bot_command_sequence: Option<(u64, u64, u64)> = None;
    let (server_tx, mut server_rx) = mpsc::channel(8);
    let _server_reader = AbortOnDrop(tokio::spawn(async move {
        loop {
            let result = read_server_frame(&mut reader, &mut dec).await;
            let ended = result.is_err();
            if server_tx.send(result).await.is_err() || ended {
                break;
            }
        }
    }));
    let ping_interval = Duration::from_secs(30);
    let mut next_ping = tokio::time::Instant::now() + ping_interval;
    let mut ping_sequence = 1_u32;
    let mut pending_ping: Option<(u32, tokio::time::Instant)> = None;

    account
        .session_tx
        .send(SessionMessage::UpstreamConnected(true))
        .await
        .ok();
    tracing::info!(account=%account.config.account_name, upstream=%addr, "headless upstream world authenticated socket established");

    loop {
        tokio::select! {
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() { return Ok(()); }
            }
            command = commands.recv() => {
                match command {
                    Ok(dispatch) => {
                        let owner = *account.ownership_state.borrow();
                        if !dispatch.is_current(owner, movement_world_generation)
                            || !dispatch.is_new_sequence(&mut last_bot_command_sequence)
                        {
                            let (generation, movement_epoch, sequence, world_generation) = match &dispatch {
                                crate::configured_session::GameplayDispatch::BotMovement { generation, movement_epoch, sequence, world_generation, .. } =>
                                    (Some(generation.get()), Some(movement_epoch.get()), Some(*sequence), Some(*world_generation)),
                                crate::configured_session::GameplayDispatch::Unfenced(_) => (None, None, None, None),
                            };
                            tracing::warn!(account=%account.config.account_name, source="bot", sequence, session="headless", ownership_generation=generation, current_generation=owner.generation.get(), movement_epoch, current_epoch=owner.movement_epoch.get(), world_generation, current_world_generation=movement_world_generation, owner=?owner.mode, attended_control=?owner.attended_control, reason="stale stamp or sequence", "dropped stale bot movement command before headless routing");
                            continue;
                        }
                        let command = dispatch.command().clone();
                        if world_transfer_pending
                            && matches!(command, GameplayCommand::MoveTo(_) | GameplayCommand::FaceDirection { .. } | GameplayCommand::StopMovement)
                        {
                            tracing::debug!(account=%account.config.account_name, ?command, "discarded locomotion command during world transfer");
                            continue;
                        }
                        let movement_context = gameplay_movement_context(
                            controlled_mover,
                            controlled_position,
                            controlled_flags,
                            player_guid,
                            canonical_position,
                            canonical_flags,
                        );
                        match encode_gameplay_command(
                            command.clone(),
                            movement_context.mover,
                            movement_context.position,
                            movement_context.flags,
                            &mut movement_clock,
                        ) {
                            Ok(Some((frame, movement))) => {
                                let bot_cast = bot_targeted_cast(&command);
                                let gameobject_report_use =
                                    gameobject_report_use_for_command(&command);
                                if let Some((spell, target)) = bot_cast {
                                    last_bot_cast = Some((spell, target, std::time::Instant::now()));
                                }
                                if let Some(target) = bot_loot_target_for_command(&command) {
                                    bot_loot_target = Some(target);
                                    bot_loot_response_seen = false;
                                }
                                shared.action_logs.packet(
                                    &account.config.account_name,
                                    "bot",
                                    "C2S",
                                    frame.opcode,
                                    &frame.body,
                                ).await;
                                log_loot_wire_packet(&account.config.account_name, "bot_to_server", frame.opcode, &frame.body);
                                let movement_guard = if dispatch.bot_movement_stamp().is_some() {
                                    Some(account.movement_gate.clone().lock_owned().await)
                                } else {
                                    None
                                };
                                if !dispatch.is_current(*account.ownership_state.borrow(), movement_world_generation) {
                                    tracing::warn!(account=%account.config.account_name, opcode=frame.opcode, "dropped bot movement command after ownership changed during headless encoding");
                                    continue;
                                }
                                write_client_frame(&mut writer, &mut enc, &frame).await?;
                                tracing::info!(account=%account.config.account_name, opcode=frame.opcode, "headless bot gameplay packet transmitted");
                                if let Some((spell, target)) = bot_cast {
                                    tracing::info!(account=%account.config.account_name, opcode=frame.opcode, spell, ?target, "headless bot cast request transmitted");
                                }
                                if let Some((target, report)) = gameobject_report_use {
                                    shared.action_logs.packet(
                                        &account.config.account_name,
                                        "bot",
                                        "C2S",
                                        report.opcode,
                                        &report.body,
                                    ).await;
                                    log_loot_wire_packet(
                                        &account.config.account_name,
                                        "bot_to_server",
                                        report.opcode,
                                        &report.body,
                                    );
                                    write_client_frame(&mut writer, &mut enc, &report).await?;
                                    tracing::info!(account=%account.config.account_name, opcode=report.opcode, ?target, "headless bot game-object report-use packet transmitted");
                                }
                                if should_publish_movement_prediction(&command)
                                    && let Some((position, moving, flags, client_time)) = movement
                                {
                                    shared.action_logs.position(
                                        &account.config.account_name,
                                        "bot_prediction",
                                        position,
                                        movement_context.mover,
                                        Some(u32::from(frame.opcode)),
                                        Some(flags),
                                        Some(client_time),
                                    ).await;
                                    if controlled_mover.is_some() {
                                        controlled_position = Some(position);
                                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::ControlledMover { mover: controlled_mover, position: Some(position), flags })).await;
                                    } else {
                                        canonical_position = Some(position);
                                        canonical_flags = flags;
                                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::PlayerPosition { position, moving, flags, client_time })).await;
                                    }
                                }
                                drop(movement_guard);
                            }
                            Ok(None) => {}
                            Err(reason) => tracing::warn!(account=%account.config.account_name, command=?command, %reason, "headless gameplay command has no encoder"),
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => tracing::warn!(account=%account.config.account_name, skipped, "headless command bus lagged"),
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            _ = tokio::time::sleep_until(next_ping) => {
                let sequence = ping_sequence;
                ping_sequence = ping_sequence.wrapping_add(1);
                let frame = make_headless_ping(sequence, 0);
                shared.action_logs.packet(
                    &account.config.account_name,
                    "proxy",
                    "C2S",
                    frame.opcode,
                    &frame.body,
                ).await;
                write_client_frame(&mut writer, &mut enc, &frame).await?;
                pending_ping = Some((sequence, tokio::time::Instant::now()));
                tracing::debug!(account=%account.config.account_name, sequence, opcode=frame.opcode, "headless client keepalive ping transmitted");
                next_ping = tokio::time::Instant::now() + ping_interval;
            }
            server = server_rx.recv() => {
                let frame = server.context("headless upstream reader ended")??;
                let opcode = u32::from(frame.opcode);
                if matches!(opcode, 0x003E | 0x0236) {
                    match fence_actor_movement(&account).await {
                        Some((epoch, generation)) => {
                            movement_world_generation = generation;
                            tracing::info!(account=%account.config.account_name, movement_epoch=epoch.get(), world_generation=generation, opcode, "headless movement fenced for world transition");
                        }
                        None => bail!("headless world-transition movement fence failed"),
                    }
                }
                shared.action_logs.packet(
                    &account.config.account_name,
                    "world",
                    "S2C",
                    opcode,
                    &frame.body,
                ).await;
                log_bot_cast_progress(
                    &account.config.account_name,
                    opcode,
                    &frame.body,
                    player_guid,
                    &last_bot_cast,
                );
                if opcode == 0x003F {
                    world_transfer_pending = true;
                    tracing::info!(account=%account.config.account_name, "headless world transfer started");
                }
                if opcode == u32::from(SMSG_FORCE_RUN_SPEED_CHANGE_OPCODE)
                    && let Some((mover, move_event, speed)) =
                        parse_force_run_speed_change(&frame.body)
                    && player_guid == Some(mover)
                {
                    if let Some(position) = canonical_position {
                        let ack = encode_force_run_speed_change_ack(
                            mover,
                            move_event,
                            speed,
                            movement_clock.next_timestamp(),
                            position,
                        );
                        shared.action_logs.packet(
                            &account.config.account_name,
                            "proxy",
                            "C2S",
                            ack.opcode,
                            &ack.body,
                        ).await;
                        write_client_frame(&mut writer, &mut enc, &ack).await?;
                        tracing::info!(
                            account=%account.config.account_name,
                            speed_yards_per_second=speed,
                            move_event,
                            opcode=ack.opcode,
                            "headless run speed update acknowledged"
                        );
                    } else {
                        tracing::warn!(
                            account=%account.config.account_name,
                            ?mover,
                            move_event,
                            "run speed update arrived before the player position was known; acknowledgement was not sent"
                        );
                    }
                }
                if frame.opcode == SMSG_TIME_SYNC_REQ_OPCODE {
                    let counter = parse_time_sync_request(&frame.body)
                        .context("SMSG_TIME_SYNC_REQ is missing its u32 counter")?;
                    let response = encode_time_sync_response(
                        counter,
                        movement_clock.current_timestamp(),
                    );
                    shared.action_logs.packet(
                        &account.config.account_name,
                        "proxy",
                        "C2S",
                        response.opcode,
                        &response.body,
                    ).await;
                    write_client_frame(&mut writer, &mut enc, &response).await?;
                    tracing::debug!(account=%account.config.account_name, counter, "headless movement time synchronization response sent");
                    continue;
                }
                if opcode == 0x00C7
                    && let Some((mover, flags, point, orientation)) = parse_server_near_teleport(&frame.body)
                    && (player_guid == Some(mover) || controlled_mover == Some(mover))
                {
                    let mut body = Vec::with_capacity(17);
                    push_packed_guid(&mut body, mover);
                    body.extend_from_slice(&flags.to_le_bytes());
                    body.extend_from_slice(&movement_clock.next_timestamp().to_le_bytes());
                    let ack = ClientFrame {
                        opcode: 0x00C7,
                        body,
                    };
                    shared.action_logs.packet(
                        &account.config.account_name,
                        "proxy",
                        "C2S",
                        ack.opcode,
                        &ack.body,
                    ).await;
                    write_client_frame(&mut writer, &mut enc, &ack).await?;
                    if let Some(mut position) = canonical_position.or(controlled_position) {
                        position.point = point;
                        position.orientation = orientation;
                        if player_guid == Some(mover) {
                            canonical_position = Some(position);
                            canonical_flags = flags;
                                    let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::PlayerPosition {
                                position,
                                moving: flags != 0,
                                flags,
                                client_time: movement_clock.current_timestamp(),
                            })).await;
                        } else if controlled_mover == Some(mover) {
                            controlled_position = Some(position);
                            controlled_flags = flags;
                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::ControlledMover {
                                mover: Some(mover),
                                position: Some(position),
                                flags,
                            })).await;
                        }
                    }
                    tracing::info!(account=%account.config.account_name, ?mover, "headless near teleport acknowledged");
                }
                if opcode == 0x01DD {
                    if let Some(sequence) = parse_headless_pong(&frame.body) {
                        if let Some((pending, sent_at)) = pending_ping.take().filter(|(pending, _)| *pending == sequence) {
                            tracing::debug!(account=%account.config.account_name, sequence=pending, rtt_ms=sent_at.elapsed().as_millis(), "headless server keepalive pong received");
                        } else {
                            tracing::debug!(account=%account.config.account_name, sequence, "headless server pong did not match pending ping");
                        }
                    }
                }
                if opcode == 0x01EE && !char_enum_requested {
                    let request = ClientFrame { opcode: 0x0037, body: Vec::new() };
                    shared.action_logs.packet(&account.config.account_name, "proxy", "C2S", request.opcode, &request.body).await;
                    write_client_frame(&mut writer, &mut enc, &request).await?;
                    char_enum_requested = true;
                    tracing::info!(account=%account.config.account_name, "headless character enumeration requested");
                    continue;
                }
                if opcode == 0x003B && !player_login_requested {
                    let (guid, name) = select_wrath_character(&frame.body, account.config.character.as_deref())
                        .context("configured headless character was not present in SMSG_CHAR_ENUM")?;
                    player_guid = Some(EntityId(guid));
                    object_observer.set_player_guid(EntityId(guid));
                    let request = ClientFrame { opcode: 0x003D, body: guid.to_le_bytes().to_vec() };
                    shared.action_logs.packet(&account.config.account_name, "proxy", "C2S", request.opcode, &request.body).await;
                    write_client_frame(&mut writer, &mut enc, &request).await?;
                    player_login_requested = true;
                    tracing::info!(account=%account.config.account_name, character=%name, guid=%format_args!("0x{guid:016X}"), "headless configured character login requested");
                    continue;
                }
                if opcode == u32::from(SMSG_WARDEN_DATA::OPCODE) {
                    let reply = warden.handle(&frame.body).context("headless Warden exchange failed")?;
                    if let Some(body) = reply.body {
                        let response = ClientFrame {
                            opcode: CMSG_WARDEN_DATA::OPCODE,
                            body,
                        };
                        shared.action_logs.packet(&account.config.account_name, "proxy", "C2S", u32::from(response.opcode), &response.body).await;
                        write_client_frame(&mut writer, &mut enc, &response).await?;
                    }
                    if reply.event != "module chunk" {
                        tracing::info!(account=%account.config.account_name, event=reply.event, "headless Warden exchange advanced");
                    }
                    continue;
                }
                log_loot_wire_packet(&account.config.account_name, "server_to_bot", opcode, &frame.body);
                if opcode == 0x0160 {
                    let packet_guid = frame.body.get(..8).and_then(|bytes| <[u8; 8]>::try_from(bytes).ok()).map(u64::from_le_bytes);
                    tracing::info!(account=%account.config.account_name, opcode, body_len=frame.body.len(), packet_guid=?packet_guid, expected_guid=?bot_loot_target, "loot response packet received");
                    if let Some(loot) = parse_loot_response(&frame.body) {
                        if bot_loot_target.is_some_and(|target| target.0 == loot.guid) {
                            bot_loot_response_seen = true;
                            if loot.loot_type == 0 {
                                let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootRejected { target: EntityId(loot.guid), loot_type: loot.loot_type, error: loot.error })).await;
                                tracing::warn!(account=%account.config.account_name, loot_guid=loot.guid, loot_type=loot.loot_type, loot_error=?loot.error, "server rejected bot-owned loot request");
                                let release = ClientFrame { opcode: 0x015F, body: loot.guid.to_le_bytes().to_vec() };
                                shared.action_logs.packet(&account.config.account_name, "proxy", "C2S", release.opcode, &release.body).await;
                                log_loot_wire_packet(&account.config.account_name, "bot_to_server", release.opcode, &release.body);
                                write_client_frame(&mut writer, &mut enc, &release).await?;
                            } else {
                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootOpened { target: EntityId(loot.guid), ownership: wow_state::observation::LootOwnership::Bot })).await;
                            tracing::info!(account=%account.config.account_name, loot_guid=loot.guid, loot_type=loot.loot_type, gold=loot.gold, slots=?loot.slots, "bot loot window opened; collecting items");
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            if loot.gold > 0 {
                                let money = ClientFrame { opcode: 0x015E, body: Vec::new() };
                                shared.action_logs.packet(&account.config.account_name, "proxy", "C2S", money.opcode, &money.body).await;
                                log_loot_wire_packet(&account.config.account_name, "bot_to_server", money.opcode, &money.body);
                                write_client_frame(&mut writer, &mut enc, &money).await?;
                            }
                            for slot in loot.slots {
                                let take = ClientFrame { opcode: 0x0108, body: vec![slot] };
                                shared.action_logs.packet(&account.config.account_name, "proxy", "C2S", take.opcode, &take.body).await;
                                log_loot_wire_packet(&account.config.account_name, "bot_to_server", take.opcode, &take.body);
                                write_client_frame(&mut writer, &mut enc, &take).await?;
                            }
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            let release = ClientFrame { opcode: 0x015F, body: loot.guid.to_le_bytes().to_vec() };
                            shared.action_logs.packet(&account.config.account_name, "proxy", "C2S", release.opcode, &release.body).await;
                            log_loot_wire_packet(&account.config.account_name, "bot_to_server", release.opcode, &release.body);
                            write_client_frame(&mut writer, &mut enc, &release).await?;
                            }
                        } else {
                            tracing::warn!(account=%account.config.account_name, received_guid=loot.guid, expected_guid=?bot_loot_target, loot_type=loot.loot_type, "loot response did not match active bot loot target");
                        }
                    } else {
                        tracing::warn!(account=%account.config.account_name, body_len=frame.body.len(), "failed to parse loot response packet");
                    }
                } else if opcode == 0x0161 && bot_loot_target.is_some() {
                    tracing::info!(account=%account.config.account_name, body_len=frame.body.len(), packet_guid=?frame.body.get(..8).and_then(|bytes| <[u8; 8]>::try_from(bytes).ok()).map(u64::from_le_bytes), expected_guid=?bot_loot_target, "loot release response received");
                    if let Some(target) = rejected_loot_target_without_response(
                        bot_loot_target,
                        bot_loot_response_seen,
                    )
                    {
                        tracing::warn!(account=%account.config.account_name, loot_guid=target.0, "server released bot loot request without a loot response");
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootRejected {
                            target,
                            loot_type: 0,
                            error: None,
                        })).await;
                    }
                    let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootClosed { ownership: wow_state::observation::LootOwnership::Bot })).await;
                    bot_loot_target = None;
                    bot_loot_response_seen = false;
                } else if matches!(opcode, 0x0161..=0x0163) {
                    tracing::info!(account=%account.config.account_name, opcode, body_len=frame.body.len(), packet_guid=?frame.body.get(..8).and_then(|bytes| <[u8; 8]>::try_from(bytes).ok()).map(u64::from_le_bytes), expected_guid=?bot_loot_target, "loot state packet received without active bot loot target");
                }
                if opcode == 0x0130 {
                    if let Some((cast_count, spell, reason, target)) =
                        take_bot_cast_failure(&frame.body, &mut last_bot_cast)
                    {
                        tracing::warn!(account=%account.config.account_name, cast_count, spell, reason, ?target, "authoritative bot cast failure observed");
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::CastFailed { spell, reason, target })).await;
                    }
                }
                if opcode == 0x003E {
                    if let Some(guid) = player_guid
                        && let Some(ProtocolObservation::WorldChanged { position, .. }) = new_world(&frame.body, guid.0)
                    {
                        canonical_position = Some(position);
                        canonical_flags = 0;
                        controlled_mover = None;
                        controlled_position = None;
                        controlled_flags = 0;
                        object_observer.change_world(position, Some(guid))?;
                        let _ = account.session_tx.send(SessionMessage::WorldAuthoritative(false)).await;
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::WorldChanged {
                            character_guid: guid.0,
                            position,
                        })).await;
                        let _ = account.session_tx.send(SessionMessage::WorldAuthoritative(true)).await;
                    }
                    // Keep movement_clock alive across the transfer. The
                    // server's clockDelta still maps this client clock to its
                    // clock for the lifetime of this world session.
                    // MSG_MOVE_WORLDPORT_ACK has an empty body. The server holds
                    // a far teleport open until this packet arrives.
                    let ack = ClientFrame {
                        opcode: 0x00DC,
                        body: Vec::new(),
                    };
                    shared.action_logs.packet(&account.config.account_name, "proxy", "C2S", ack.opcode, &ack.body).await;
                    write_client_frame(&mut writer, &mut enc, &ack).await?;
                    world_transfer_pending = false;
                    tracing::info!(account=%account.config.account_name, opcode, "headless world transfer acknowledged");
                }
                if opcode == 0x0236 {
                    if let Some(guid) = player_guid {
                        if let Some(observation) = login_verify_world(&frame.body, guid.0) {
                            if let ProtocolObservation::EnteredWorld { position: Some(position), .. } = &observation {
                                object_observer.set_world(*position, Some(guid))?;
                                canonical_position = Some(*position);
                                canonical_flags = 0;
                            }
                            let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                            let _ = account.session_tx.send(SessionMessage::WorldAuthoritative(true)).await;
                            let mission_id = MissionId(account.mission_counter.fetch_add(1, Ordering::Relaxed));
                            let mission = Mission::quest(mission_id);
                            if let Err(error) = account.supervisor_tx.send(SupervisorCommand::ReplaceMission { lane: account.config.lane, mission }).await {
                                tracing::error!(account=%account.config.account_name, %error, "failed to install default headless Quest mission");
                            } else {
                                tracing::info!(account=%account.config.account_name, ?mission_id, "default headless Quest mission installed after authoritative world entry");
                            }
                            let (committed, receiver) = oneshot::channel();
                            if account.session_tx.send(SessionMessage::BotOn { committed: Some(committed) }).await.is_ok() {
                                if receiver.await != Ok(true) {
                                    tracing::warn!(account=%account.config.account_name, "headless bot ON ownership change did not commit");
                                }
                            }
                            tracing::info!(account=%account.config.account_name, ?guid, "headless configured character entered world authoritatively; Quest mission and bot ownership requested");
                        }
                    }
                }
                forward_protocol_observations(&account, &account.config.account_name, opcode, &frame.body).await;
                match object_observer.observe(frame.opcode, &frame.body).await {
                    Ok(observations) => for observation in observations {
                        if let ProtocolObservation::ControlledMover { mover, position, flags } = &observation {
                            controlled_mover = *mover;
                            controlled_position = *position;
                            controlled_flags = *flags;
                            if let Some(position) = position {
                                shared.action_logs.position(
                                    &account.config.account_name,
                                    "server_controlled_mover",
                                    *position,
                                    *mover,
                                    Some(opcode),
                                    Some(*flags),
                                    None,
                                ).await;
                            }
                        }
                        let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                    },
                    Err(error) => tracing::warn!(account=%account.config.account_name, opcode, %error, "headless Tentacli observation failed"),
                }
            }
        }
    }
}

fn select_wrath_character(body: &[u8], configured: Option<&str>) -> Option<(u64, String)> {
    const FIXED_TAIL_AFTER_NAME: usize = 9 + 4 + 4 + 12 + 4 + 4 + 4 + 1 + 4 + 4 + 4 + 23 * 9;
    let (&count, mut rest) = body.split_first()?;
    let mut first = None;
    for _ in 0..count {
        let guid = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?);
        rest = rest.get(8..)?;
        let end = rest.iter().position(|byte| *byte == 0)?;
        let name = std::str::from_utf8(rest.get(..end)?).ok()?.to_owned();
        rest = rest.get(end + 1..)?;
        if first.is_none() {
            first = Some((guid, name.clone()));
        }
        if configured.is_some_and(|wanted| name.eq_ignore_ascii_case(wanted)) {
            return Some((guid, name));
        }
        rest = rest.get(FIXED_TAIL_AFTER_NAME..)?;
    }
    if configured.is_none_or(|name| name.trim().is_empty()) {
        first
    } else {
        None
    }
}

fn bot_targeted_cast(command: &GameplayCommand) -> Option<(u32, Option<EntityId>)> {
    match command {
        GameplayCommand::Cast { spell, target }
        | GameplayCommand::VehicleCast { spell, target } => Some((*spell, *target)),
        GameplayCommand::CastOnItem { spell, item_guid } => Some((*spell, Some(*item_guid))),
        GameplayCommand::MaintainBuff { spell, target } => Some((*spell, Some(*target))),
        GameplayCommand::SummonPet { spell, player } => Some((*spell, Some(*player))),
        GameplayCommand::CastGameObject { spell, target, .. } => Some((*spell, Some(*target))),
        GameplayCommand::UseItemInstance { spell, target, .. } if *spell != 0 => {
            Some((*spell, *target))
        }
        _ => None,
    }
}

fn log_bot_cast_progress(
    account_name: &str,
    opcode: u32,
    body: &[u8],
    player_guid: Option<EntityId>,
    last_bot_cast: &Option<(u32, Option<EntityId>, std::time::Instant)>,
) {
    let response = maintenance_observations(opcode, body)
        .into_iter()
        .find_map(|observation| match observation {
            ProtocolObservation::CastStarted {
                caster,
                spell,
                ends_at_ms,
                ..
            } => Some(("started", caster, spell, Some(ends_at_ms))),
            ProtocolObservation::CastFinished { caster, spell } => {
                Some(("finished", caster, spell, None))
            }
            _ => None,
        });
    let Some((event, caster, spell, ends_at_ms)) = response else {
        return;
    };
    let pending = last_bot_cast
        .as_ref()
        .filter(|(pending_spell, _, sent_at)| {
            *pending_spell == spell && sent_at.elapsed() < Duration::from_secs(5)
        });
    if player_guid != Some(caster) && pending.is_none() {
        return;
    }
    let target = pending.and_then(|(_, target, _)| *target);
    let pending_age_ms = pending.map(|(_, _, sent_at)| sent_at.elapsed().as_millis());
    tracing::info!(
        account = account_name,
        opcode,
        ?caster,
        spell,
        ?target,
        ?pending_age_ms,
        ?ends_at_ms,
        event,
        "authoritative spell cast progress"
    );
}

fn should_publish_movement_prediction(command: &GameplayCommand) -> bool {
    !matches!(command, GameplayCommand::FaceDirection { .. })
}

#[derive(Clone, Copy)]
struct GameplayMovementContext {
    mover: Option<EntityId>,
    position: Option<WorldPosition>,
    flags: u32,
}

fn gameplay_movement_context(
    controlled_mover: Option<EntityId>,
    controlled_position: Option<WorldPosition>,
    controlled_flags: u32,
    player_guid: Option<EntityId>,
    player_position: Option<WorldPosition>,
    player_flags: u32,
) -> GameplayMovementContext {
    GameplayMovementContext {
        mover: controlled_mover.or(player_guid),
        position: controlled_position.or(player_position),
        flags: if controlled_mover.is_some() {
            controlled_flags
        } else {
            player_flags
        },
    }
}

#[cfg(test)]
mod action_log_position_tests {
    use super::*;

    #[tokio::test]
    async fn action_log_records_position_coordinates_and_source() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = PathBuf::from("/private/tmp").join(format!("wow-proxy-position-log-{unique}"));
        let logs = ActionLogManager::new(dir.clone());
        let path = logs.start("TEST1", None).await.expect("log should start");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        logs.position(
            "TEST1",
            "bot_prediction",
            WorldPosition {
                map: 0,
                point: Vec3::new(1.25, -2.5, 3.75),
                orientation: 0.5,
            },
            Some(EntityId(42)),
            Some(238),
            Some(1),
            Some(1234),
        )
        .await;
        logs.stop("TEST1").await.expect("log should stop");

        let contents = std::fs::read_to_string(path).expect("log should be readable");
        let position_record = contents
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|record| record["event"] == "position")
            .expect("position record should exist");
        assert_eq!(position_record["source"], "bot_prediction");
        assert_eq!(position_record["map"], 0);
        assert_eq!(position_record["x"], 1.25);
        assert_eq!(position_record["y"], -2.5);
        assert_eq!(position_record["z"], 3.75);
        assert_eq!(position_record["mover"], 42);
        std::fs::remove_dir_all(dir).expect("temporary log directory should be removed");
    }
}

#[cfg(test)]
mod action_log_redaction_tests {
    use super::*;

    #[test]
    fn action_logs_redact_chat_and_warden_bodies() {
        assert_eq!(
            action_log_redaction_reason("C2S", 0x0095),
            Some("chat payload redacted")
        );
        assert_eq!(
            action_log_redaction_reason("S2C", u32::from(SMSG_WARDEN_DATA::OPCODE)),
            Some("Warden payload redacted")
        );
        assert_eq!(action_log_redaction_reason("C2S", 0x00B5), None);
    }
}

#[cfg(test)]
mod bot_loot_target_tests {
    use super::*;

    #[test]
    fn tracks_corpse_loot_and_game_object_use_as_bot_loot_sources() {
        let target = EntityId(42);
        assert_eq!(
            bot_loot_target_for_command(&GameplayCommand::Loot(target)),
            Some(target)
        );
        assert_eq!(
            bot_loot_target_for_command(&GameplayCommand::UseGameObject(target)),
            Some(target)
        );
        assert_eq!(
            bot_loot_target_for_command(&GameplayCommand::CastGameObject {
                spell: QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID,
                target,
                report_use: true,
            }),
            Some(target)
        );
        assert_eq!(
            bot_loot_target_for_command(&GameplayCommand::CastGameObject {
                spell: QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID,
                target,
                report_use: false,
            }),
            None
        );
        assert_eq!(
            bot_loot_target_for_command(&GameplayCommand::CastGameObject {
                spell: 6247,
                target,
                report_use: true,
            }),
            None
        );
        assert_eq!(
            bot_loot_target_for_command(&GameplayCommand::Interact(target)),
            None
        );
    }

    #[test]
    fn quest_item_open_emits_the_client_gameobject_report_use_packet() {
        let target = EntityId(0xF110_0244_1300_0C92);
        let (reported_target, frame) =
            gameobject_report_use_for_command(&GameplayCommand::CastGameObject {
                spell: QUEST_ITEM_GAMEOBJECT_OPEN_SPELL_ID,
                target,
                report_use: true,
            })
            .expect("quest item chest cast must report its use");

        assert_eq!(reported_target, target);
        assert_eq!(frame.opcode, 0x0481);
        assert_eq!(frame.body, target.0.to_le_bytes());
        assert!(
            gameobject_report_use_for_command(&GameplayCommand::CastGameObject {
                spell: 6247,
                target,
                report_use: false,
            })
            .is_none()
        );
    }

    #[test]
    fn release_without_loot_response_is_a_failed_open_but_successful_open_is_not() {
        let target = EntityId(42);
        assert_eq!(
            rejected_loot_target_without_response(Some(target), false),
            Some(target)
        );
        assert_eq!(
            rejected_loot_target_without_response(Some(target), true),
            None
        );
        assert_eq!(rejected_loot_target_without_response(None, false), None);
    }
}

#[cfg(test)]
mod gameplay_movement_context_tests {
    use super::*;

    #[test]
    fn controlled_mover_state_takes_precedence_with_player_fallback() {
        let player_position = WorldPosition {
            map: 1,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 0.5,
        };
        let context = gameplay_movement_context(
            Some(EntityId(2)),
            None,
            0x20,
            Some(EntityId(1)),
            Some(player_position),
            0x10,
        );

        assert_eq!(context.mover, Some(EntityId(2)));
        assert_eq!(context.position, Some(player_position));
        assert_eq!(context.flags, 0x20);
    }

    #[test]
    fn player_state_is_used_without_a_controlled_mover() {
        let player_position = WorldPosition {
            map: 1,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 0.5,
        };
        let context = gameplay_movement_context(
            None,
            None,
            0x20,
            Some(EntityId(1)),
            Some(player_position),
            0x10,
        );

        assert_eq!(context.mover, Some(EntityId(1)));
        assert_eq!(context.position, Some(player_position));
        assert_eq!(context.flags, 0x10);
    }

    #[test]
    fn facing_request_does_not_publish_a_predicted_position() {
        assert!(!should_publish_movement_prediction(
            &GameplayCommand::FaceDirection { orientation: 1.0 }
        ));
        assert!(should_publish_movement_prediction(
            &GameplayCommand::MoveTo(Vec3::new(1.0, 2.0, 3.0))
        ));
    }
}

fn make_headless_ping(sequence: u32, latency_ms: u32) -> ClientFrame {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&sequence.to_le_bytes());
    body.extend_from_slice(&latency_ms.to_le_bytes());
    ClientFrame {
        opcode: 0x01DC,
        body,
    }
}

fn parse_headless_pong(body: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(body.get(..4)?.try_into().ok()?))
}

async fn forward_protocol_observations(
    account: &AccountRuntime,
    account_name: &str,
    opcode: u32,
    body: &[u8],
) {
    if opcode == u32::from(SMSG_FORCE_RUN_SPEED_CHANGE_OPCODE)
        && let Some((_, _, yards_per_second)) = parse_force_run_speed_change(body)
    {
        tracing::info!(
            account = account_name,
            yards_per_second,
            "authoritative player run speed observed"
        );
        let _ = account
            .session_tx
            .send(SessionMessage::Observation(
                ProtocolObservation::RunSpeedChanged { yards_per_second },
            ))
            .await;
    }
    for observation in quest_observations(opcode, body) {
        tracing::debug!(
            account = account_name,
            ?observation,
            "authoritative quest observation"
        );
        let _ = account
            .session_tx
            .send(SessionMessage::Observation(observation))
            .await;
    }
    for observation in maintenance_observations(opcode, body) {
        tracing::debug!(
            account = account_name,
            ?observation,
            "authoritative maintenance observation"
        );
        let _ = account
            .session_tx
            .send(SessionMessage::Observation(observation))
            .await;
    }
    if let Some(observation) = controlled_abilities_observation(opcode, body) {
        tracing::info!(
            account = account_name,
            ?observation,
            "authoritative controlled-unit abilities observed"
        );
        let _ = account
            .session_tx
            .send(SessionMessage::Observation(observation))
            .await;
    }
}

fn proxy_chat(body: &[u8]) -> Option<(crate::commands::chat::ChatFamily, &str)> {
    use crate::commands::chat::ChatFamily;
    let chat_type = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
    let mut offset = 8;
    let family = match chat_type {
        1 => ChatFamily::Say,
        2 | 51 => ChatFamily::Party,
        3 | 39 => ChatFamily::Raid,
        4 | 5 => ChatFamily::Guild,
        6 => ChatFamily::Yell,
        7 => ChatFamily::Whisper,
        10 => ChatFamily::Emote,
        40 => ChatFamily::RaidWarning,
        44 | 45 => ChatFamily::Battleground,
        17 | 23 | 24 => ChatFamily::Say,
        _ => ChatFamily::Other,
    };
    let text = match chat_type {
        1 | 2 | 3 | 4 | 5 | 6 | 10 | 23 | 24 | 39 | 40 | 44 | 45 | 51 => {
            protocol_observations::read_cstring(body, &mut offset)?
        }
        7 | 17 => {
            protocol_observations::read_cstring(body, &mut offset)?;
            protocol_observations::read_cstring(body, &mut offset)?
        }
        _ => return None,
    };
    Some((family, text))
}

fn is_afk_chat_request(frame: &ClientFrame) -> bool {
    frame.opcode == 0x0095
        && frame.body.len() >= 9
        && frame.body.get(..4) == Some(23_u32.to_le_bytes().as_slice())
}

async fn execute_terminal_bot_command(
    account: &AccountRuntime,
    text: &str,
) -> Result<Vec<String>, String> {
    let mission_id = MissionId(account.mission_counter.fetch_add(1, Ordering::Relaxed));
    match crate::commands::parse_local(
        text,
        crate::commands::chat::ChatFamily::Say,
        true,
        mission_id,
    )
    .map_err(|error| format!("invalid bot command: {error}"))?
    {
        Some(crate::commands::LocalCommand::Bot(command)) => {
            execute_bot_command(account, command).await
        }
        _ => Err("unknown .bot command".into()),
    }
}

async fn execute_bot_command(
    account: &AccountRuntime,
    command: crate::commands::bot::BotCommand,
) -> Result<Vec<String>, String> {
    use crate::commands::bot::BotCommand;

    if account.config.worker.is_none()
        && matches!(&command, BotCommand::On | BotCommand::Mission(_))
    {
        return Err("this account has no enabled bot worker".into());
    }

    match command {
        BotCommand::On => {
            let _ = account.visual_fence.send(());
            let (committed, receiver) = oneshot::channel();
            account
                .session_tx
                .send(SessionMessage::BotOn {
                    committed: Some(committed),
                })
                .await
                .map_err(|error| format!("could not enable bot control: {error}"))?;
            if !receiver
                .await
                .map_err(|error| format!("bot control result was lost: {error}"))?
            {
                return Err("bot control did not start; check the upstream connection".into());
            }
            tracing::info!(account=%account.config.account_name, command=".bot on", "bot automation enabled");
            Ok(vec!["automation enabled".into()])
        }
        BotCommand::Off => {
            let _ = account.visual_fence.send(());
            let (committed, receiver) = oneshot::channel();
            account
                .session_tx
                .send(SessionMessage::BotOff {
                    committed: Some(committed),
                })
                .await
                .map_err(|error| format!("could not disable bot control: {error}"))?;
            if !receiver
                .await
                .map_err(|error| format!("bot control result was lost: {error}"))?
            {
                return Err("bot control did not stop".into());
            }
            tracing::info!(account=%account.config.account_name, command=".bot off", "bot automation disabled");
            Ok(vec!["automation disabled".into()])
        }
        BotCommand::Mission(mission) => {
            let _ = account.visual_fence.send(());
            let description = format!("{:?}", mission.intent);
            account
                .supervisor_tx
                .send(SupervisorCommand::ReplaceMission {
                    lane: account.config.lane,
                    mission,
                })
                .await
                .map_err(|error| format!("failed to route bot mission: {error}"))?;
            let (committed, receiver) = oneshot::channel();
            account
                .session_tx
                .send(SessionMessage::BotOn {
                    committed: Some(committed),
                })
                .await
                .map_err(|error| {
                    format!("mission was installed, but bot control could not start: {error}")
                })?;
            if !receiver.await.map_err(|error| {
                format!("mission was installed, but bot control result was lost: {error}")
            })? {
                return Err("mission was installed, but bot control did not start; check the upstream connection".into());
            }
            tracing::info!(account=%account.config.account_name, mission=%description, "mission installed and bot automation enabled");
            Ok(vec![format!(
                "mission set: {description}; automation enabled"
            )])
        }
        BotCommand::Status => {
            let owner = *account.ownership_state.borrow();
            let character = account
                .config
                .character
                .as_deref()
                .filter(|name| !name.is_empty())
                .unwrap_or(&account.config.account_name);
            let status = format!(
                "{character}: mode={:?}, requested={:?}, phase={:?}, attendance={:?}, bot_allowed={}",
                owner.mode,
                owner.requested,
                owner.phase,
                owner.attendance,
                owner.bot_allowed(),
            );
            tracing::info!(account=%account.config.account_name, %status, "bot status requested");
            Ok(vec![status])
        }
        BotCommand::Help => Ok(crate::commands::bot::HELP_LINES
            .iter()
            .map(|line| (*line).to_owned())
            .collect()),
    }
}

async fn handle_log_command(
    logs: &ActionLogManager,
    account: &str,
    command: crate::commands::log::LogCommand,
) -> String {
    use crate::commands::log::LogCommand;
    match command {
        LogCommand::Start(label) => match logs.start(account, label.as_deref()).await {
            Ok(path) => {
                tracing::info!(account=%account, path=%path.display(), "action logging started");
                format!(
                    "[wow-bot] action and movement-position logging started: {}",
                    path.display()
                )
            }
            Err(error) => {
                tracing::error!(account=%account, %error, "failed to start action logging");
                format!("[wow-bot] failed to start action logging: {error}")
            }
        },
        LogCommand::Stop => match logs.stop(account).await {
            Ok(Some((path, failure))) => {
                tracing::info!(account=%account, path=%path.display(), "action logging stopped");
                match failure {
                    Some(error) => format!(
                        "[wow-bot] action logging stopped INCOMPLETE: {} ({error})",
                        path.display()
                    ),
                    None => format!("[wow-bot] action logging stopped: {}", path.display()),
                }
            }
            Ok(None) => {
                tracing::info!(account=%account, "action logging is not active");
                "[wow-bot] action logging is not active".to_string()
            }
            Err(error) => {
                tracing::error!(account=%account, %error, "failed to stop action logging");
                format!("[wow-bot] failed to stop action logging: {error}")
            }
        },
        LogCommand::Status => match logs.status(account).await {
            Some((path, Some(error))) => {
                tracing::warn!(account=%account, path=%path.display(), %error, "action logging is incomplete");
                format!(
                    "[wow-bot] action logging is INCOMPLETE: {} ({error})",
                    path.display()
                )
            }
            Some((path, None)) => {
                tracing::info!(account=%account, path=%path.display(), "action logging is active");
                format!("[wow-bot] action logging is active: {}", path.display())
            }
            None => {
                tracing::info!(account=%account, "action logging is inactive");
                "[wow-bot] action logging is inactive".to_string()
            }
        },
        LogCommand::Mark(label) => match logs.mark(account, label.as_deref()).await {
            Ok(true) => {
                tracing::info!(account=%account, ?label, "action log marker written");
                "[wow-bot] action log marker written".to_string()
            }
            Ok(false) => {
                tracing::info!(account=%account, "action logging is not active; marker ignored");
                "[wow-bot] action logging is not active; marker ignored".to_string()
            }
            Err(error) => {
                tracing::error!(account=%account, %error, "failed to write action log marker");
                format!("[wow-bot] failed to write action log marker: {error}")
            }
        },
        LogCommand::Help => crate::commands::bot::HELP_LINES
            .iter()
            .find(|line| line.starts_with(".log"))
            .copied()
            .unwrap_or(
                ".log start [label] | .log mark [label] | .log status | .log stop | .log help",
            )
            .to_owned(),
    }
}

fn port_of(bind: &str) -> Result<u16> {
    Ok(bind.parse::<SocketAddr>()?.port())
}
fn advertised(host: &str, port: u16) -> String {
    wow_domain::endpoint::Endpoint {
        host: host.to_owned(),
        port,
    }
    .authority()
}

#[cfg(test)]
mod runtime_chat_tests {
    use super::*;
    use crate::commands::{self, LocalCommand};

    #[test]
    fn advertised_formats_ipv4_hostname_and_ipv6_authorities() {
        assert_eq!(advertised("192.0.2.1", 3724), "192.0.2.1:3724");
        assert_eq!(
            advertised("world.example.test", 3724),
            "world.example.test:3724"
        );
        assert_eq!(advertised("2001:db8::1", 3724), "[2001:db8::1]:3724");
    }

    #[test]
    fn local_clients_get_local_realm_host_and_remote_clients_get_public_host() {
        let lan_client = IpAddr::from([10, 0, 0, 222]);
        let remote_client = IpAddr::from([68, 44, 101, 193]);

        assert_eq!(
            realm_advertise_host(lan_client, "10.0.0.133", "game.example.test"),
            "10.0.0.133"
        );
        assert_eq!(
            realm_advertise_host(remote_client, "10.0.0.133", "game.example.test"),
            "game.example.test"
        );
        assert_eq!(
            advertised(
                realm_advertise_host(lan_client, "10.0.0.133", "game.example.test"),
                3725
            ),
            "10.0.0.133:3725"
        );
        assert_eq!(
            advertised(
                realm_advertise_host(remote_client, "10.0.0.133", "game.example.test"),
                3725
            ),
            "game.example.test:3725"
        );
    }

    fn chat_body(chat_type: u32, target: Option<&str>, message: &str) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&chat_type.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        if let Some(target) = target {
            body.extend_from_slice(target.as_bytes());
            body.push(0);
        }
        body.extend_from_slice(message.as_bytes());
        body.push(0);
        body
    }

    #[test]
    fn bot_on_and_off_are_extracted_from_say_chat() {
        for text in [".bot on", ".bot off"] {
            let body = chat_body(1, None, text);
            let (family, extracted) = proxy_chat(&body).expect("chat should parse");
            assert_eq!(extracted, text);
            let parsed = commands::parse_local(extracted, family, true, MissionId(1)).unwrap();
            assert!(matches!(parsed, Some(LocalCommand::Bot(_))));
        }
    }

    #[test]
    fn local_commands_are_extracted_from_whisper_and_channel() {
        for (chat_type, target) in [(7, "Target"), (17, "General")] {
            let body = chat_body(chat_type, Some(target), ".log status");
            let (family, extracted) = proxy_chat(&body).expect("chat should parse");
            assert_eq!(extracted, ".log status");
            assert!(matches!(
                commands::parse_local(extracted, family, true, MissionId(1)).unwrap(),
                Some(LocalCommand::Log(_))
            ));
        }
    }

    #[test]
    fn matching_bot_facing_echo_does_not_need_human_takeover() {
        let visual = WorldPosition {
            map: 1,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 1.25,
        };
        assert!(movement_matches_bot_visual(
            visual,
            EntityId(7),
            0x0DA,
            EntityId(7),
            visual.point,
            1.25,
            0x0DA,
            Duration::ZERO,
        ));
        assert!(!movement_matches_bot_visual(
            visual,
            EntityId(7),
            0x0DA,
            EntityId(8),
            Vec3::new(4.0, 2.0, 3.0),
            1.25,
            0x0DA,
            Duration::ZERO,
        ));
    }

    #[test]
    fn mirrored_movement_echo_with_client_adjusted_opcode_does_not_take_over() {
        let visual = WorldPosition {
            map: 1,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 1.25,
        };
        assert!(movement_matches_bot_visual(
            visual,
            EntityId(7),
            0x0B5,
            EntityId(7),
            Vec3::new(8.5, 2.0, 3.0),
            1.55,
            0x0BC,
            Duration::from_secs(1),
        ));
        assert!(!movement_matches_bot_visual(
            visual,
            EntityId(7),
            0x0B5,
            EntityId(7),
            Vec3::new(10.1, 2.0, 3.0),
            1.25,
            0x0BC,
            Duration::from_millis(1250),
        ));
    }

    #[test]
    fn takeover_validation_requires_matching_guid_and_opcode_flags() {
        assert!(validate_player_takeover(0x0B5, EntityId(7), 0x01, Some(EntityId(7))).is_ok());
        assert!(validate_player_takeover(0x0B5, EntityId(8), 0x01, Some(EntityId(7))).is_err());
        assert!(validate_player_takeover(0x0B5, EntityId(7), 0x02, Some(EntityId(7))).is_err());
        assert!(validate_player_takeover(0x0B5, EntityId(7), 0x03, Some(EntityId(7))).is_err());
        assert!(validate_player_takeover(0x0B7, EntityId(7), 0, Some(EntityId(7))).is_err());
    }

    #[test]
    fn movement_mirror_rejects_stale_session_generation_epoch_and_sequence() {
        let stamp = MovementStamp {
            source: MovementVisualSource::Bot,
            session_id: 3,
            sequence: 4,
            ownership_generation: 5,
            movement_epoch: 6,
            route_epoch: 8,
            world_generation: 7,
        };
        let mirror = MovementMirror {
            stamp,
            frame: crate::framing::ServerFrame {
                opcode: 1,
                body: Vec::new(),
            },
        };
        let is_current = |stamp, session, owner, epoch, route, world, last| {
            movement_visual_is_current(
                stamp,
                MovementVisualSource::Bot,
                session,
                owner,
                epoch,
                route,
                world,
                last,
            )
        };
        assert!(is_current(mirror.stamp, 3, 5, 6, 8, 7, 3));
        assert!(!is_current(mirror.stamp, 9, 5, 6, 8, 7, 3));
        assert!(!is_current(mirror.stamp, 3, 8, 6, 8, 7, 3));
        assert!(!is_current(mirror.stamp, 3, 5, 8, 8, 7, 3));
        assert!(!is_current(mirror.stamp, 3, 5, 6, 9, 7, 3));
        assert!(!is_current(mirror.stamp, 3, 5, 6, 8, 8, 3));
        assert!(!is_current(mirror.stamp, 3, 5, 6, 8, 7, 4));
        assert!(!movement_visual_is_current(
            mirror.stamp,
            MovementVisualSource::Player,
            3,
            5,
            6,
            8,
            7,
            3
        ));
        assert!(movement_visual_is_current(
            MovementStamp {
                source: MovementVisualSource::Player,
                ..mirror.stamp
            },
            MovementVisualSource::Player,
            3,
            5,
            6,
            8,
            7,
            3,
        ));
        assert!(movement_visual_is_current(
            MovementStamp {
                source: MovementVisualSource::Server,
                ..mirror.stamp
            },
            MovementVisualSource::Server,
            3,
            5,
            6,
            8,
            7,
            3,
        ));
        assert!(movement_visual_is_current(
            MovementStamp {
                source: MovementVisualSource::ProxyHandoff,
                ..mirror.stamp
            },
            MovementVisualSource::ProxyHandoff,
            3,
            5,
            6,
            8,
            7,
            3,
        ));
    }

    #[test]
    fn takeover_invalidation_removes_queued_movement_visual() {
        let stamp = MovementStamp {
            source: MovementVisualSource::Bot,
            session_id: 3,
            sequence: 4,
            ownership_generation: 5,
            movement_epoch: 6,
            route_epoch: 8,
            world_generation: 7,
        };
        let mirror = MovementMirror {
            stamp,
            frame: crate::framing::ServerFrame {
                opcode: 1,
                body: Vec::new(),
            },
        };
        let (updates, mut receiver) = watch::channel(Some(mirror.clone()));
        let mut route_epoch = 8;
        invalidate_movement_mirror(&mut route_epoch, &updates);
        assert!(receiver.borrow_and_update().is_none());
        assert!(!movement_visual_is_current(
            mirror.stamp,
            MovementVisualSource::Bot,
            3,
            5,
            6,
            route_epoch,
            7,
            3,
        ));
    }

    #[test]
    fn movement_sequences_increase_for_each_source_and_server_visuals_are_classified() {
        let mut sequence = 0;
        assert_eq!(next_movement_sequence(&mut sequence), 1);
        assert_eq!(next_movement_sequence(&mut sequence), 2);
        assert_eq!(next_movement_sequence(&mut sequence), 3);
        assert!(is_server_movement_visual_opcode(0x00C7));
        assert!(is_server_movement_visual_opcode(0x00DD));
        assert!(is_server_movement_visual_opcode(0x02AE));
        assert!(!is_server_movement_visual_opcode(0x00A9));
        let position = WorldPosition {
            map: 0,
            point: Vec3::default(),
            orientation: 0.0,
        };
        assert!(is_movement_observation(
            &ProtocolObservation::PlayerPosition {
                position,
                moving: true,
                flags: 1,
                client_time: 2,
            }
        ));
        assert!(is_movement_observation(
            &ProtocolObservation::ControlledMover {
                mover: Some(EntityId(7)),
                position: Some(position),
                flags: 1,
            }
        ));
        assert!(!is_movement_observation(&ProtocolObservation::LeftWorld));
    }

    #[test]
    fn near_teleport_ack_requires_a_matching_pending_correction() {
        let mut body = Vec::new();
        push_packed_guid(&mut body, EntityId(7));
        body.extend_from_slice(&0x20_u32.to_le_bytes());
        body.extend_from_slice(&99_u32.to_le_bytes());

        assert_eq!(
            validate_near_teleport_ack(&body, Some((EntityId(7), 0x20)), Some(EntityId(7)), None,),
            Ok((EntityId(7), 0x20, 99)),
        );
        assert!(validate_near_teleport_ack(&body, None, Some(EntityId(7)), None).is_err());
        assert!(
            validate_near_teleport_ack(&body, Some((EntityId(8), 0x20)), Some(EntityId(7)), None)
                .is_err()
        );
        assert!(
            validate_near_teleport_ack(&body, Some((EntityId(7), 0x21)), Some(EntityId(7)), None)
                .is_err()
        );
        assert!(
            validate_near_teleport_ack(
                &body[..body.len() - 1],
                Some((EntityId(7), 0x20)),
                Some(EntityId(7)),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn passive_movement_feedback_does_not_count_as_player_intent() {
        for opcode in [
            0x0B7, 0x0BA, 0x0BE, 0x0C1, 0x0C2, 0x0C3, 0x0C9, 0x0CB, 0x0EE, 0x35A,
        ] {
            assert!(is_player_movement_opcode(opcode));
            assert!(
                !is_explicit_player_movement_intent(opcode),
                "opcode {opcode:#x} must remain passive"
            );
        }
        // Server correction acknowledgements use a separate forwarding path.
        for opcode in [0x0C7, 0x0E3, 0x0E5, 0x0F0, 0x2CF, 0x517] {
            assert!(!is_player_movement_opcode(opcode));
            assert!(!is_explicit_player_movement_intent(opcode));
        }
    }

    #[test]
    fn explicit_start_turn_jump_and_facing_packets_take_player_control() {
        for opcode in [
            0x0B5, 0x0B6, 0x0B8, 0x0B9, 0x0BB, 0x0BC, 0x0BD, 0x0BF, 0x0C0, 0x0CA, 0x0DA, 0x0DB,
            0x359, 0x3A7,
        ] {
            assert!(is_player_movement_opcode(opcode));
            assert!(
                is_explicit_player_movement_intent(opcode),
                "opcode {opcode:#x} must count as explicit intent"
            );
        }
    }
}

#[cfg(test)]
mod player_world_handoff_tests {
    use super::*;

    #[test]
    fn missing_warden_image_is_a_non_retryable_configuration_error() {
        let error = anyhow::anyhow!("headless Warden exchange failed")
            .context("Warden checks require proxy.warden_client_image");
        assert!(is_missing_warden_client_image(&error));

        let transient = anyhow::anyhow!("connection reset");
        assert!(!is_missing_warden_client_image(&transient));
    }

    #[test]
    fn headless_waits_for_final_overlapping_player_connection() {
        let worlds = Arc::new(Mutex::new(PlayerWorlds::default()));
        let (enabled, receiver) = watch::channel(true);
        let first = PlayerWorldHandoff::begin(worlds.clone(), enabled.clone());
        let second = PlayerWorldHandoff::begin(worlds.clone(), enabled);
        assert_ne!(first.connection, second.connection);
        assert!(!*receiver.borrow());

        drop(first);
        assert!(!*receiver.borrow());

        drop(second);
        assert!(*receiver.borrow());
    }

    #[test]
    fn failed_player_setup_restores_headless_when_no_player_remains() {
        let worlds = Arc::new(Mutex::new(PlayerWorlds::default()));
        let (enabled, receiver) = watch::channel(true);
        {
            let _handoff = PlayerWorldHandoff::begin(worlds, enabled);
            assert!(!*receiver.borrow());
        }
        assert!(*receiver.borrow());
    }
}

#[cfg(test)]
mod quest_protocol_tests {
    use super::*;

    #[test]
    fn parses_azerothcore_vehicle_action_bar_spells() {
        let mut body = Vec::new();
        body.extend_from_slice(&28511_u64.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&0x08000000_u32.to_le_bytes());
        body.extend_from_slice(&(51858_u32 | (8_u32 << 24)).to_le_bytes());
        for _ in 1..10 {
            body.extend_from_slice(&0_u32.to_le_bytes());
        }
        body.push(0);
        body.push(0);
        let obs = controlled_abilities_observation(0x0179, &body).expect("vehicle spell packet");
        assert!(
            matches!(obs, ProtocolObservation::ControlledAbilities { mover: EntityId(28511), ref spells } if spells == &vec![51858])
        );
    }

    #[test]
    fn encodes_gameobject_spell_with_gameobject_target() {
        let mut movement_clock = MovementClock::default();
        let (frame, movement) = encode_gameplay_command(
            GameplayCommand::CastGameObject {
                spell: 6247,
                target: EntityId(191609),
                report_use: true,
            },
            None,
            None,
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frame.opcode, 0x012E);
        assert!(movement.is_none());
        assert_eq!(frame.body[0], 0);
        assert_eq!(
            u32::from_le_bytes(frame.body[1..5].try_into().unwrap()),
            6247
        );
        assert_eq!(
            u32::from_le_bytes(frame.body[6..10].try_into().unwrap()),
            0x0000_0800
        );
    }

    #[test]
    fn movement_starts_once_then_uses_heartbeats_until_stop() {
        let mut movement_clock = MovementClock::default();
        let position = WorldPosition {
            map: 0,
            point: Vec3::new(0.0, 0.0, 0.0),
            orientation: 0.0,
        };
        let (start, state) = encode_gameplay_command(
            GameplayCommand::MoveTo(Vec3::new(0.7, 0.0, 0.0)),
            Some(EntityId(1)),
            Some(position),
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(start.opcode, 0x00B5);
        let (position, moving, flags, _) = state.unwrap();
        assert!(moving);
        let (heartbeat, state) = encode_gameplay_command(
            GameplayCommand::MoveTo(Vec3::new(1.4, 0.0, 0.0)),
            Some(EntityId(1)),
            Some(position),
            flags,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(heartbeat.opcode, 0x00EE);
        let (position, _, flags, _) = state.unwrap();
        let (stop, _) = encode_gameplay_command(
            GameplayCommand::StopMovement,
            Some(EntityId(1)),
            Some(position),
            flags,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(stop.opcode, 0x00B7);
    }

    #[test]
    fn face_direction_uses_wrath_set_facing_movement_opcode() {
        let mut movement_clock = MovementClock::default();
        let current = WorldPosition {
            map: 1,
            point: Vec3::new(1.0, 2.0, 3.0),
            orientation: 0.0,
        };
        let (frame, movement) = encode_gameplay_command(
            GameplayCommand::FaceDirection {
                orientation: std::f32::consts::PI,
            },
            Some(EntityId(77)),
            Some(current),
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frame.opcode, 0x00DA);
        let (position, moving, _, _) = movement.expect("facing updates canonical movement state");
        assert!(!moving);
        assert!((position.orientation - std::f32::consts::PI).abs() < 1.0e-5);
    }

    #[test]
    fn targeted_spell_correlation_covers_item_and_controlled_casts() {
        assert_eq!(
            bot_targeted_cast(&GameplayCommand::VehicleCast {
                spell: 51858,
                target: Some(EntityId(2))
            }),
            Some((51858, Some(EntityId(2))))
        );
        assert_eq!(
            bot_targeted_cast(&GameplayCommand::UseItemInstance {
                item: 16114,
                item_guid: EntityId(9),
                backpack_slot: 3,
                spell: 19938,
                target: Some(EntityId(4)),
                cast_count: 0
            }),
            Some((19938, Some(EntityId(4))))
        );
        assert_eq!(
            bot_targeted_cast(&GameplayCommand::CastGameObject {
                spell: 6247,
                target: EntityId(5),
                report_use: true
            }),
            Some((6247, Some(EntityId(5))))
        );
    }

    #[test]
    fn parses_azerothcore_cast_failed_layout() {
        let mut body = Vec::new();
        body.push(3);
        body.extend_from_slice(&51858_u32.to_le_bytes());
        body.push(47);
        assert_eq!(parse_cast_failed(&body), Some((3, 51858, 47)));
    }

    #[test]
    fn parses_initial_spellbook_and_aura_updates_for_maintenance() {
        let mut spells = vec![0_u8];
        spells.extend_from_slice(&2_u16.to_le_bytes());
        spells.extend_from_slice(&1459_u32.to_le_bytes());
        spells.extend_from_slice(&0_u16.to_le_bytes());
        spells.extend_from_slice(&1243_u32.to_le_bytes());
        spells.extend_from_slice(&0_u16.to_le_bytes());
        let observed = parse_initial_spells(&spells);
        assert!(
            observed
                .iter()
                .any(|o| matches!(o, ProtocolObservation::SpellKnown { spell: 1459 }))
        );
        assert!(
            observed
                .iter()
                .any(|o| matches!(o, ProtocolObservation::SpellKnown { spell: 1243 }))
        );

        let mut aura = Vec::new();
        push_packed_guid(&mut aura, EntityId(7));
        aura.push(3);
        aura.extend_from_slice(&1459_u32.to_le_bytes());
        aura.extend_from_slice(&[0x20, 1, 1]); // caster and duration follow
        push_packed_guid(&mut aura, EntityId(9));
        aura.extend_from_slice(&30_000_u32.to_le_bytes());
        aura.extend_from_slice(&12_000_u32.to_le_bytes());
        assert!(matches!(
            parse_aura_update(&aura),
            Some(ProtocolObservation::AuraSlot {
                entity: EntityId(7),
                slot: 3,
                aura: Some(wow_state::auras::AuraInstance {
                    spell: 1459,
                    caster: Some(EntityId(9)),
                    max_duration_ms: Some(30_000),
                    remaining_ms: Some(12_000),
                    observed_at_ms: Some(_),
                    ..
                })
            })
        ));

        let mut ghost_aura = Vec::new();
        push_packed_guid(&mut ghost_aura, EntityId(7));
        ghost_aura.push(0);
        ghost_aura.extend_from_slice(&wow_state::life::GHOST_AURA_SPELL_ID.to_le_bytes());
        ghost_aura.extend_from_slice(&[0x08, 1, 0]); // caster omitted, level, stacks
        assert!(matches!(
            parse_aura_update(&ghost_aura),
            Some(ProtocolObservation::AuraSlot {
                entity: EntityId(7),
                slot: 0,
                aura: Some(wow_state::auras::AuraInstance {
                    spell: wow_state::life::GHOST_AURA_SPELL_ID,
                    ..
                })
            })
        ));

        let mut all_auras = Vec::new();
        push_packed_guid(&mut all_auras, EntityId(7));
        all_auras.push(3);
        all_auras.extend_from_slice(&172_u32.to_le_bytes());
        all_auras.extend_from_slice(&[0x20, 1, 2]);
        push_packed_guid(&mut all_auras, EntityId(9));
        all_auras.extend_from_slice(&18_000_u32.to_le_bytes());
        all_auras.extend_from_slice(&8_000_u32.to_le_bytes());
        assert!(matches!(
            parse_aura_update_all(&all_auras),
            Some(ProtocolObservation::AuraSnapshot { auras, .. })
                if matches!(auras.as_slice(), [wow_state::auras::AuraInstance {
                    slot: 3,
                    spell: 172,
                    positive: Some(false),
                    caster: Some(EntityId(9)),
                    max_duration_ms: Some(18_000),
                    remaining_ms: Some(8_000),
                    observed_at_ms: Some(_),
                }])
        ));

        let mut removed = Vec::new();
        push_packed_guid(&mut removed, EntityId(7));
        removed.push(3);
        removed.extend_from_slice(&0_u32.to_le_bytes());
        assert!(matches!(
            parse_aura_update(&removed),
            Some(ProtocolObservation::AuraSlot {
                entity: EntityId(7),
                slot: 3,
                aura: None
            })
        ));
    }

    #[test]
    fn encodes_controlled_unit_spell_with_unit_target() {
        let mut movement_clock = MovementClock::default();
        let (frame, movement) = encode_gameplay_command(
            GameplayCommand::VehicleCast {
                spell: 51858,
                target: Some(EntityId(28525)),
            },
            Some(EntityId(28511)),
            None,
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frame.opcode, 0x012E);
        assert!(movement.is_none());
        assert_eq!(frame.body[0], 0);
        assert_eq!(
            u32::from_le_bytes(frame.body[1..5].try_into().unwrap()),
            51858
        );
        assert_eq!(u32::from_le_bytes(frame.body[6..10].try_into().unwrap()), 2);
    }

    #[test]
    fn parses_azerothcore_multiple_quest_status_layout() {
        let mut body = Vec::new();
        body.extend_from_slice(&2_u32.to_le_bytes());
        body.extend_from_slice(&11_u64.to_le_bytes());
        body.push(8);
        body.extend_from_slice(&22_u64.to_le_bytes());
        body.push(5);
        let observations = parse_multiple_quest_status(&body);
        assert!(matches!(
            observations.as_slice(),
            [
                ProtocolObservation::QuestGiverStatus {
                    giver: EntityId(11),
                    status: 8
                },
                ProtocolObservation::QuestGiverStatus {
                    giver: EntityId(22),
                    status: 5
                },
            ]
        ));
    }

    #[test]
    fn parses_azerothcore_quest_list_layout() {
        let mut body = Vec::new();
        body.extend_from_slice(&99_u64.to_le_bytes());
        body.extend_from_slice(b"Greetings\0");
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.push(1);
        body.extend_from_slice(&1234_u32.to_le_bytes());
        body.extend_from_slice(&2_u32.to_le_bytes());
        body.extend_from_slice(&10_i32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.push(0);
        body.extend_from_slice(b"A Test Quest\0");
        let observations = parse_quest_list(&body);
        assert!(matches!(
            observations.as_slice(),
            [
                ProtocolObservation::QuestGiverListReceived {
                    giver: EntityId(99),
                    offer_count: 1
                },
                ProtocolObservation::QuestOffer {
                    giver: EntityId(99),
                    quest: 1234,
                    icon: 2
                }
            ]
        ));
    }

    #[test]
    fn parses_empty_quest_giver_list_as_response_evidence() {
        let mut body = Vec::new();
        body.extend_from_slice(&99_u64.to_le_bytes());
        body.extend_from_slice(b"Greetings\0");
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.push(0);
        assert!(matches!(
            parse_quest_list(&body).as_slice(),
            [ProtocolObservation::QuestGiverListReceived {
                giver: EntityId(99),
                offer_count: 0
            }]
        ));
    }

    #[test]
    fn headless_keepalive_uses_wrath_ping_fields() {
        let ping = make_headless_ping(0x1234_5678, 42);
        assert_eq!(ping.opcode, 0x01DC);
        assert_eq!(ping.body, [0x78, 0x56, 0x34, 0x12, 42, 0, 0, 0]);
        assert_eq!(
            parse_headless_pong(&0x1234_5678_u32.to_le_bytes()),
            Some(0x1234_5678)
        );
        assert_eq!(parse_headless_pong(&[1, 2]), None);
    }

    #[test]
    fn parses_azerothcore_quest_query_response_objectives() {
        let mut fields = vec![0_u32; 65];
        fields[0] = 1234;
        fields[61] = 0;
        fields[62] = 100.0_f32.to_bits();
        fields[63] = 200.0_f32.to_bits();
        let mut body = Vec::new();
        for value in fields {
            body.extend_from_slice(&value.to_le_bytes());
        }
        for text in ["Quest Title", "Objectives", "Details", "Area", "Complete"] {
            body.extend_from_slice(text.as_bytes());
            body.push(0);
        }
        // Four fixed NPC/GO objective slots. Preserve their original slot indexes.
        body.extend_from_slice(&77_u32.to_le_bytes());
        body.extend_from_slice(&3_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&(88_u32 | 0x8000_0000).to_le_bytes());
        body.extend_from_slice(&1_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        for _ in 0..2 {
            body.extend_from_slice(&0_u32.to_le_bytes());
            body.extend_from_slice(&0_u32.to_le_bytes());
            body.extend_from_slice(&0_u32.to_le_bytes());
            body.extend_from_slice(&0_u32.to_le_bytes());
        }
        // Six item objective slots.
        body.extend_from_slice(&99_u32.to_le_bytes());
        body.extend_from_slice(&4_u32.to_le_bytes());
        for _ in 0..5 {
            body.extend_from_slice(&0_u32.to_le_bytes());
            body.extend_from_slice(&0_u32.to_le_bytes());
        }
        for text in ["Kill wolves", "Use object", "", ""] {
            body.extend_from_slice(text.as_bytes());
            body.push(0);
        }

        let observation = parse_quest_query_response(&body).expect("quest definition should parse");
        let ProtocolObservation::QuestDefinition { definition } = observation else {
            panic!("wrong observation");
        };
        assert_eq!(definition.quest, 1234);
        assert_eq!(definition.title, "Quest Title");
        assert_eq!(definition.poi_map, Some(0));
        assert_eq!(definition.targets.len(), 2);
        assert_eq!(definition.targets[0].slot, 0);
        assert_eq!(definition.targets[0].entry, 77);
        assert_eq!(definition.targets[1].slot, 1);
        assert!(matches!(
            definition.targets[1].kind,
            wow_state::quests::QuestTargetKind::GameObject
        ));
        assert_eq!(definition.items.len(), 1);
        assert_eq!(definition.items[0].item, 99);
        assert_eq!(definition.items[0].required, 4);
    }

    #[test]
    fn parses_wrath_loot_response_slots() {
        let guid = 0x1122_3344_5566_7788_u64;
        let mut body = Vec::new();
        body.extend_from_slice(&guid.to_le_bytes());
        body.push(1); // loot type
        body.extend_from_slice(&7_u32.to_le_bytes()); // money
        body.push(3); // item count
        for slot in [1_u8, 2, 3] {
            body.push(slot);
            body.extend_from_slice(&100_u32.to_le_bytes()); // item id
            body.extend_from_slice(&1_u32.to_le_bytes()); // count
            body.extend_from_slice(&200_u32.to_le_bytes()); // display id
            body.extend_from_slice(&0_u32.to_le_bytes());
            body.extend_from_slice(&0_u32.to_le_bytes());
            body.push(0);
        }
        assert_eq!(body.len(), 80);
        let parsed = parse_loot_response(&body).expect("loot response should parse");
        assert_eq!(parsed.guid, guid);
        assert_eq!(parsed.loot_type, 1);
        assert_eq!(parsed.error, None);
        assert_eq!(parsed.gold, 7);
        assert_eq!(parsed.slots, vec![1, 2, 3]);
    }

    #[test]
    fn parses_rejected_wrath_loot_response_type() {
        let guid = 0x1122_3344_5566_7788_u64;
        let mut body = Vec::new();
        body.extend_from_slice(&guid.to_le_bytes());
        body.push(0); // server rejected the loot request
        body.push(12); // LOOT_ERROR_MASTER_INV_FULL

        assert_eq!(
            parse_loot_response(&body),
            Some(LootResponse {
                guid,
                loot_type: 0,
                error: Some(12),
                gold: 0,
                slots: Vec::new(),
            })
        );
    }

    #[test]
    fn parses_sanitized_azerothcore_loot_capture_fixtures() {
        let fixtures = [
            (
                include_str!("../testdata/smsg_loot_response_two_items.hex"),
                0xF130_000C_3400_834E,
                vec![0, 1],
            ),
            (
                include_str!("../testdata/smsg_loot_response_three_items.hex"),
                0xF130_000C_3400_AA70,
                vec![0, 1, 2],
            ),
        ];

        for (capture, expected_guid, expected_slots) in fixtures {
            let body = decode_hex_fixture(capture);
            assert_eq!(body.len(), 14 + expected_slots.len() * 22);
            let parsed =
                parse_loot_response(&body).expect("captured Wrath loot response should parse");
            assert_eq!(parsed.guid, expected_guid);
            assert_eq!(parsed.loot_type, 1);
            assert_eq!(parsed.gold, 0);
            assert_eq!(parsed.slots, expected_slots);
        }
    }

    fn decode_hex_fixture(hex: &str) -> Vec<u8> {
        let hex = hex.trim();
        assert_eq!(hex.len() % 2, 0, "hex fixture must contain whole bytes");
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).expect("hex is ASCII");
                u8::from_str_radix(pair, 16).expect("fixture contains valid hex")
            })
            .collect()
    }

    #[test]
    fn parses_azerothcore_turn_in_dialogs() {
        let giver = 77_u64;
        let quest = 42_u32;

        let mut request = Vec::new();
        request.extend_from_slice(&giver.to_le_bytes());
        request.extend_from_slice(&quest.to_le_bytes());
        request.extend_from_slice(b"Title\0Items\0");
        request.extend_from_slice(&[0_u8; 28]);
        let n = request.len();
        request[n - 16..n - 12].copy_from_slice(&3_u32.to_le_bytes());
        let parsed = parse_quest_request_items(&request).expect("request-items should parse");
        assert!(matches!(
            parsed,
            ProtocolObservation::QuestTurnInDialog {
                quest: 42,
                dialog: wow_state::quests::QuestTurnInDialog {
                    giver: EntityId(77),
                    stage: wow_state::quests::QuestTurnInStage::RequestItems { can_complete: true }
                }
            }
        ));

        let mut offer = Vec::new();
        offer.extend_from_slice(&giver.to_le_bytes());
        offer.extend_from_slice(&quest.to_le_bytes());
        offer.extend_from_slice(b"Title\0Reward\0");
        offer.push(1);
        offer.extend_from_slice(&0_u32.to_le_bytes());
        offer.extend_from_slice(&0_u32.to_le_bytes());
        offer.extend_from_slice(&0_u32.to_le_bytes());
        offer.extend_from_slice(&2_u32.to_le_bytes());
        offer.extend_from_slice(&1001_u32.to_le_bytes());
        offer.extend_from_slice(&1_u32.to_le_bytes());
        offer.extend_from_slice(&501_u32.to_le_bytes());
        offer.extend_from_slice(&1002_u32.to_le_bytes());
        offer.extend_from_slice(&1_u32.to_le_bytes());
        offer.extend_from_slice(&502_u32.to_le_bytes());
        let parsed = parse_quest_offer_reward(&offer).expect("offer-reward should parse");
        let ProtocolObservation::QuestTurnInDialog { quest, dialog } = parsed else {
            panic!("expected quest turn-in offer")
        };
        assert_eq!(quest, 42);
        assert_eq!(dialog.giver, EntityId(77));
        assert_eq!(
            dialog.stage,
            wow_state::quests::QuestTurnInStage::OfferReward {
                reward_items: vec![1001, 1002]
            }
        );
        offer.pop();
        assert!(parse_quest_offer_reward(&offer).is_none());
    }

    #[test]
    fn quest_encoder_matches_azerothcore_handlers() {
        let mut movement_clock = MovementClock::default();
        let (query, _) = encode_gameplay_command(
            GameplayCommand::QueryQuestGivers,
            None,
            None,
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(query.opcode, 0x0417);
        assert!(query.body.is_empty());

        let mut movement_clock = MovementClock::default();
        let (hello, _) = encode_gameplay_command(
            GameplayCommand::Interact(EntityId(77)),
            None,
            None,
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(hello.opcode, 0x0184);
        assert_eq!(hello.body, 77_u64.to_le_bytes());

        let mut movement_clock = MovementClock::default();
        let (accept, _) = encode_gameplay_command(
            GameplayCommand::AcceptQuest {
                quest: 42,
                giver: EntityId(77),
            },
            None,
            None,
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(accept.opcode, 0x0189);
        assert_eq!(accept.body.len(), 16);
        assert_eq!(&accept.body[0..8], &77_u64.to_le_bytes());
        assert_eq!(&accept.body[8..12], &42_u32.to_le_bytes());
        assert_eq!(&accept.body[12..16], &0_u32.to_le_bytes());

        let mut movement_clock = MovementClock::default();
        let (request_reward, _) = encode_gameplay_command(
            GameplayCommand::RequestQuestReward {
                quest: 42,
                giver: EntityId(77),
            },
            None,
            None,
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(request_reward.opcode, 0x018C);
        assert_eq!(&request_reward.body[0..8], &77_u64.to_le_bytes());
        assert_eq!(&request_reward.body[8..12], &42_u32.to_le_bytes());

        let mut movement_clock = MovementClock::default();
        let (choose_reward, _) = encode_gameplay_command(
            GameplayCommand::ChooseQuestReward {
                quest: 42,
                giver: EntityId(77),
                reward: 0,
            },
            None,
            None,
            0,
            &mut movement_clock,
        )
        .unwrap()
        .unwrap();
        assert_eq!(choose_reward.opcode, 0x018E);
        assert_eq!(&choose_reward.body[12..16], &0_u32.to_le_bytes());
    }

    #[test]
    fn afk_filter_matches_only_the_afk_messagechat_subtype() {
        let mut afk_body = 23_u32.to_le_bytes().to_vec();
        afk_body.extend_from_slice(&0_u32.to_le_bytes());
        afk_body.push(0);
        assert!(is_afk_chat_request(&ClientFrame {
            opcode: 0x0095,
            body: afk_body,
        }));
        assert!(!is_afk_chat_request(&ClientFrame {
            opcode: 0x0095,
            body: [
                1_u32.to_le_bytes().as_slice(),
                0_u32.to_le_bytes().as_slice()
            ]
            .concat(),
        }));
        assert!(!is_afk_chat_request(&ClientFrame {
            opcode: 0x0096,
            body: [
                23_u32.to_le_bytes().as_slice(),
                0_u32.to_le_bytes().as_slice()
            ]
            .concat(),
        }));
    }
}
