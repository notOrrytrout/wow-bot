mod gameplay;
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
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::Write as _,
    net::SocketAddr,
    path::PathBuf,
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
    AccountId, EntityId, GameplayCommand, LaneId, Mission, MissionId, WorkerGeneration,
    WorldPosition,
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
use wow_tentacli_adapter::runtime::{ObjectObservationRuntime, login_verify_world};
use wow_world_messages::Message as _;
use wow_world_messages::wrath::{
    CMSG_AUTH_SESSION, CMSG_WARDEN_DATA, ClientMessage as _, SMSG_AUTH_CHALLENGE, SMSG_WARDEN_DATA,
    ServerMessage as _, tokio_expect_client_message as world_expect_client_message,
    tokio_expect_server_message as world_expect_server_message,
};

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
    pub worker: WorkerGeneration,
    pub account_name: String,
    pub password: String,
    pub character: Option<String>,
}

pub struct ManagedLane {
    pub config: ProxyAccountConfig,
    pub worker_rx: mpsc::Receiver<WorkerToProxy>,
    pub worker_tx: mpsc::Sender<ProxyToWorker>,
    pub supervisor_tx: mpsc::Sender<SupervisorCommand>,
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
struct AccountRuntime {
    config: ProxyAccountConfig,
    session_tx: mpsc::Sender<SessionMessage>,
    command_bus: broadcast::Sender<GameplayCommand>,
    supervisor_tx: mpsc::Sender<SupervisorCommand>,
    mission_counter: Arc<AtomicU64>,
    headless_enabled: watch::Sender<bool>,
    headless_active: Arc<std::sync::atomic::AtomicBool>,
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
    keys: Arc<RwLock<HashMap<String, [u8; 40]>>>,
}
impl AuthRegistry {
    async fn put(&self, account: &str, key: [u8; 40]) {
        self.keys.write().await.insert(account.to_uppercase(), key);
    }
    async fn get(&self, account: &str) -> Option<[u8; 40]> {
        self.keys.read().await.get(&account.to_uppercase()).copied()
    }
}

#[derive(Clone)]
struct ActionLogManager {
    dir: PathBuf,
    sessions: Arc<tokio::sync::Mutex<HashMap<String, ActionLogSession>>>,
}

struct ActionLogSession {
    path: PathBuf,
    file: File,
}

impl ActionLogManager {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    async fn start(&self, account: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let safe = account
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>();
        let path = self.dir.join(format!("action-{safe}-{stamp}.jsonl"));
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(
            file,
            "{{\"event\":\"start\",\"account\":{:?},\"unix_s\":{stamp}}}",
            account
        )?;
        file.flush()?;
        self.sessions.lock().await.insert(
            account.to_uppercase(),
            ActionLogSession {
                path: path.clone(),
                file,
            },
        );
        Ok(path)
    }

    async fn stop(&self, account: &str) -> Result<Option<PathBuf>> {
        let mut sessions = self.sessions.lock().await;
        if let Some(mut session) = sessions.remove(&account.to_uppercase()) {
            writeln!(session.file, "{{\"event\":\"stop\"}}")?;
            session.file.flush()?;
            return Ok(Some(session.path));
        }
        Ok(None)
    }

    async fn status(&self, account: &str) -> Option<PathBuf> {
        self.sessions
            .lock()
            .await
            .get(&account.to_uppercase())
            .map(|s| s.path.clone())
    }

    async fn mark(&self, account: &str, label: Option<&str>) -> Result<bool> {
        let mut sessions = self.sessions.lock().await;
        let Some(session) = sessions.get_mut(&account.to_uppercase()) else {
            return Ok(false);
        };
        let label = label.unwrap_or("mark").replace('"', "'");
        writeln!(session.file, "{{\"event\":\"mark\",\"label\":{:?}}}", label)?;
        session.file.flush()?;
        Ok(true)
    }

    async fn packet(&self, account: &str, direction: &str, opcode: u32, body: &[u8]) {
        let mut sessions = self.sessions.lock().await;
        let Some(session) = sessions.get_mut(&account.to_uppercase()) else {
            return;
        };
        let fingerprint = body.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        let _ = writeln!(
            session.file,
            "{{\"event\":\"packet\",\"direction\":{:?},\"opcode\":{},\"body_len\":{},\"fingerprint\":{:?}}}",
            direction,
            opcode,
            body.len(),
            format!("{fingerprint:016X}")
        );
        let _ = session.file.flush();
    }
}

#[derive(Clone)]
struct SharedRuntime {
    config: Arc<ProxyRuntimeConfig>,
    accounts: Arc<HashMap<String, AccountRuntime>>,
    auth: AuthRegistry,
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
        let headless_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let state =
            ConfiguredSessionState::new(lane.config.account, lane.config.lane, lane.config.worker);
        let supervisor_for_runtime = lane.supervisor_tx.clone();
        session_actors.push(tokio::spawn(
            ConfiguredSessionActor {
                state,
                rx: session_rx,
                worker_tx: lane.worker_tx,
                upstream_tx: command_tx.clone(),
                supervisor_tx: lane.supervisor_tx,
                diagnostics: diagnostics.clone(),
            }
            .run(),
        ));
        let session = session_tx.clone();
        tokio::spawn(async move {
            let mut worker_rx = lane.worker_rx;
            while let Some(message) = worker_rx.recv().await {
                if session.send(SessionMessage::Worker(message)).await.is_err() {
                    break;
                }
            }
        });
        let bus = command_bus.clone();
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                let _ = bus.send(command);
            }
        });
        accounts.insert(
            lane.config.account_name.to_uppercase(),
            AccountRuntime {
                config: lane.config,
                session_tx,
                command_bus,
                supervisor_tx: supervisor_for_runtime,
                mission_counter: Arc::new(AtomicU64::new(1)),
                headless_enabled,
                headless_active,
                player_worlds: Arc::new(Mutex::new(PlayerWorlds::default())),
            },
        );
    }
    let action_logs = ActionLogManager::new(config.log_dir.clone());
    let shared = SharedRuntime {
        config: Arc::new(config),
        accounts: Arc::new(accounts),
        auth: AuthRegistry::default(),
        action_logs,
    };
    for account in shared.accounts.values().cloned() {
        let shared_for_headless = shared.clone();
        tokio::spawn(async move {
            headless_session_manager(shared_for_headless, account).await;
        });
    }
    let auth = tokio::spawn(serve_auth(shared.clone()));
    let world = tokio::spawn(serve_configured_world(shared.clone()));
    let transparent = tokio::spawn(serve_transparent_world(shared.clone()));
    tokio::select! {
        r = auth => r??,
        r = world => r??,
        r = transparent => r??,
        _ = tokio::signal::ctrl_c() => {}
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
    Ok(())
}

async fn serve_auth(shared: SharedRuntime) -> Result<()> {
    let listener = TcpListener::bind(&shared.config.auth_bind).await?;
    let limiter = AdmissionController::new(AdmissionLimits {
        max_total: shared.config.max_pre_auth_connections,
        max_per_ip: shared.config.max_pre_auth_connections_per_ip,
    });
    tracing::info!(bind=%shared.config.auth_bind, "player auth listener ready");
    loop {
        let (stream, peer) = listener.accept().await?;
        let permit = match limiter.try_acquire(peer.ip()) {
            Ok(p) => p,
            Err(reason) => {
                tracing::warn!(%peer, reason, "rejected pre-auth connection");
                continue;
            }
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let result = tokio::time::timeout(
                shared.config.handshake_timeout,
                handle_auth(shared.clone(), stream),
            )
            .await;
            drop(permit);
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(%peer, error=%format_args!("{e:#}"), "auth connection failed")
                }
                Err(_) => tracing::warn!(%peer, "auth handshake timed out"),
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

async fn handle_auth(shared: SharedRuntime, mut downstream: TcpStream) -> Result<()> {
    let challenge = read_stock_challenge(&mut downstream).await?;
    let key = challenge.account.to_uppercase();
    if let Some(account) = shared.accounts.get(&key) {
        let upstream =
            upstream_login(&upstream_config(&shared, account), &account.config.password).await?;
        let authenticated = crate::auth::login::terminate_configured_downstream(
            &mut downstream,
            &ConfiguredCredential {
                account: account.config.account_name.clone(),
                password: account.config.password.clone(),
            },
        )
        .await?;
        shared.auth.put(&key, authenticated.session_key).await;
        serve_configured_realms(&shared, &mut downstream, &upstream.realm_name).await
    } else {
        transparent_auth(shared, challenge, downstream).await
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
) -> Result<()> {
    let address = advertised(
        &shared.config.advertise_host,
        port_of(&shared.config.world_bind)?,
    );
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
) -> Result<()> {
    let addr = advertised(
        &shared.config.upstream_auth_host,
        shared.config.upstream_auth_port,
    );
    let mut upstream = TcpStream::connect(&addr).await?;
    upstream.write_all(&challenge.raw).await?;
    // Relay challenge and proof without deriving the unknown account session key.
    let response = tokio_expect_server_message_protocol::<
        wow_login_messages::version_8::CMD_AUTH_LOGON_CHALLENGE_Server,
        _,
    >(&mut upstream, ProtocolVersion::Eight)
    .await?;
    response.tokio_write(&mut downstream).await?;
    let proof = tokio_expect_client_message_protocol::<
        wow_login_messages::version_8::CMD_AUTH_LOGON_PROOF_Client,
        _,
    >(&mut downstream, ProtocolVersion::Eight)
    .await?;
    proof.tokio_write(&mut upstream).await?;
    let response = tokio_expect_server_message_protocol::<
        wow_login_messages::version_8::CMD_AUTH_LOGON_PROOF_Server,
        _,
    >(&mut upstream, ProtocolVersion::Eight)
    .await?;
    response.tokio_write(&mut downstream).await?;
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
                    &shared.config.advertise_host,
                    port_of(&shared.config.transparent_world_bind)?,
                );
            }
        }
        realms.tokio_write(&mut downstream).await?;
    }
    Ok(())
}

async fn serve_transparent_world(shared: SharedRuntime) -> Result<()> {
    let listener = TcpListener::bind(&shared.config.transparent_world_bind).await?;
    tracing::info!(bind=%shared.config.transparent_world_bind, "transparent world listener ready");
    loop {
        let (mut downstream, peer) = listener.accept().await?;
        let addr = advertised(
            &shared.config.upstream_world_host,
            shared.config.upstream_world_port,
        );
        tokio::spawn(async move {
            match TcpStream::connect(&addr).await {
                Ok(mut upstream) => match transparent_relay(&mut downstream, &mut upstream).await {
                    Ok((up, down)) => {
                        tracing::debug!(%peer, up, down, "transparent world relay ended")
                    }
                    Err(e) => tracing::warn!(%peer, %e, "transparent world relay failed"),
                },
                Err(e) => {
                    tracing::warn!(%peer, %e, upstream=%addr, "transparent world connect failed")
                }
            }
        });
    }
}

async fn serve_configured_world(shared: SharedRuntime) -> Result<()> {
    let listener = TcpListener::bind(&shared.config.world_bind).await?;
    tracing::info!(bind=%shared.config.world_bind, "configured world listener ready");
    loop {
        let (stream, peer) = listener.accept().await?;
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = configured_world(shared.clone(), stream).await {
                tracing::warn!(%peer, error=%format_args!("{e:#}"), "configured world bridge ended");
            }
        });
    }
}

async fn configured_world(shared: SharedRuntime, mut downstream: TcpStream) -> Result<()> {
    let seed = ProofSeed::new();
    SMSG_AUTH_CHALLENGE {
        unknown1: 1,
        server_seed: seed.seed(),
        seed: [0_u8; 32],
    }
    .tokio_write_unencrypted_server(&mut downstream)
    .await?;
    let auth = world_expect_client_message::<CMSG_AUTH_SESSION, _>(&mut downstream).await?;
    let account_name = auth.username.to_uppercase();
    let account = shared
        .accounts
        .get(&account_name)
        .with_context(|| format!("world account {account_name} is not configured"))?
        .clone();
    let handoff = PlayerWorldHandoff::begin(
        account.player_worlds.clone(),
        account.headless_enabled.clone(),
    );
    wait_for_headless_state(&account, false, Duration::from_secs(6)).await?;
    let downstream_key = shared
        .auth
        .get(&account_name)
        .await
        .context("no fresh downstream login session key for configured world connection")?;
    let normalized = NormalizedString::new(&account_name)?;
    let downstream_crypto = seed.into_server_header_crypto(
        &normalized,
        downstream_key,
        auth.client_proof,
        auth.client_seed,
    )?;
    let (mut down_enc, mut down_dec) = downstream_crypto.split();

    let upstream_login = upstream_login(
        &upstream_config(&shared, &account),
        &account.config.password,
    )
    .await?;
    let addr = advertised(&upstream_login.world_host, upstream_login.world_port);
    let mut upstream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("connect upstream world {addr}"))?;
    let challenge = world_expect_server_message::<SMSG_AUTH_CHALLENGE, _>(&mut upstream).await?;
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
    if !paused
        .await
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
    let (mut up_enc, mut up_dec) = up_crypto.split();
    let mut warden = WardenBridge::new(&upstream_login.session_key, &downstream_key);

    let mut commands = account.command_bus.subscribe();
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
    let mut last_bot_visual: Option<(WorldPosition, u32, std::time::Instant)> = None;
    let mut bot_loot_target: Option<EntityId> = None;
    let mut last_bot_cast: Option<(u32, Option<EntityId>, std::time::Instant)> = None;
    let (mut dr, mut dw) = downstream.into_split();
    let (mut ur, mut uw) = upstream.into_split();
    let result: Result<()> = loop {
        tokio::select! {
            client = read_client_frame(&mut dr, &mut down_dec) => {
                let mut frame = match client { Ok(v)=>v, Err(e)=>break Err(e) };
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
                if is_player_movement_opcode(frame.opcode) {
                    let now = std::time::Instant::now();
                    let decoded = decode_simple_movement(&frame.body);
                    let bot_locomotion_recent = last_bot_visual
                        .is_some_and(|(_, _, at)| at.elapsed() < Duration::from_millis(1250));
                    let matching_bot_echo = match (last_bot_visual, decoded) {
                        (Some((visual, bot_opcode, at)), Some((_, _, _, point, orientation))) if at.elapsed() < Duration::from_millis(1250) => {
                            movement_matches_bot_visual(visual, bot_opcode, point, orientation, frame.opcode)
                        }
                        _ => false,
                    };
                    let explicit_player_intent = is_explicit_player_movement_intent(frame.opcode);

                    if matching_bot_echo {
                        suppress_client_movement_upstream = true;
                        tracing::trace!(account=%account_name, opcode=frame.opcode, "suppressed matching bot-authored movement/facing echo");
                    } else if explicit_player_intent {
                        // A non-echoed start/turn/jump/facing opcode is actual player intent.
                        tracing::info!(account=%account_name, opcode=frame.opcode, "explicit player movement intent observed; taking locomotion from bot");
                        last_bot_visual = None;
                        let _ = account.session_tx.send(SessionMessage::PlayerMovement { at: now }).await;
                    } else if bot_locomotion_recent {
                        // AzerothCore intentionally does not echo the mover's movement
                        // packet back to that same player. We mirror bot movement locally
                        // for the attended client, which can cause the stock client to emit
                        // passive heartbeat/stop feedback. That feedback is not new human
                        // intent and must not fence the bot or overwrite its upstream path.
                        suppress_client_movement_upstream = true;
                        tracing::trace!(account=%account_name, opcode=frame.opcode, "suppressed passive client movement feedback during bot locomotion");
                    }

                    if !suppress_client_movement_upstream {
                        if let Some((guid, flags, client_time, point, orientation)) = decoded {
                            if player_guid.is_none() || player_guid == Some(guid) {
                                if let Some(mut current) = canonical_position {
                                    current.point = point;
                                    current.orientation = orientation;
                                    canonical_position = Some(current);
                                    canonical_flags = flags;
                                    movement_clock.observe(client_time);
                                    let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::PlayerPosition { position: current, moving: flags != 0, flags, client_time })).await;
                                }
                            }
                        }
                    }
                }
                if frame.opcode == 0x0095 {
                    if let Some((family, text)) = proxy_chat(&frame.body) {
                        if crate::commands::chat::is_local_namespace(text) {
                            tracing::info!(account=%account_name, ?family, command=%text, "local proxy chat command received");
                        }
                        let mission_id = MissionId(account.mission_counter.fetch_add(1, Ordering::Relaxed));
                        match crate::commands::parse_local(text, family, true, mission_id) {
                            Ok(Some(crate::commands::LocalCommand::Bot(crate::commands::bot::BotCommand::On))) => {
                                match account.session_tx.send(SessionMessage::BotOn).await {
                                    Ok(()) => {
                                        tracing::info!(account=%account_name, command=".bot on", "bot automation enabled");
                                        let _ = write_bot_notice(&mut dw, &mut down_enc, "[wow-bot] automation enabled").await;
                                    }
                                    Err(error) => tracing::error!(account=%account_name, %error, "failed to enable bot automation"),
                                }
                                continue;
                            }
                            Ok(Some(crate::commands::LocalCommand::Bot(crate::commands::bot::BotCommand::Off))) => {
                                match account.session_tx.send(SessionMessage::BotOff).await {
                                    Ok(()) => {
                                        tracing::info!(account=%account_name, command=".bot off", "bot automation disabled");
                                        let _ = write_bot_notice(&mut dw, &mut down_enc, "[wow-bot] automation disabled").await;
                                    }
                                    Err(error) => tracing::error!(account=%account_name, %error, "failed to disable bot automation"),
                                }
                                continue;
                            }
                            Ok(Some(crate::commands::LocalCommand::Bot(crate::commands::bot::BotCommand::Mission(mission)))) => {
                                let description = format!("{:?}", mission.intent);
                                match account.supervisor_tx.send(SupervisorCommand::ReplaceMission { lane: account.config.lane, mission }).await {
                                    Ok(()) => {
                                        tracing::info!(account=%account_name, mission=%description, "bot mission accepted");
                                        let notice = format!("[wow-bot] mission set: {description}");
                                        let _ = write_bot_notice(&mut dw, &mut down_enc, &notice).await;
                                    }
                                    Err(error) => tracing::error!(account=%account_name, %error, "failed to route bot mission"),
                                }
                                continue;
                            }
                            Ok(Some(crate::commands::LocalCommand::Bot(crate::commands::bot::BotCommand::Status))) => {
                                tracing::info!(account=%account_name, "bot status requested from chat; use the ownership messages below to inspect current control state");
                                continue;
                            }
                            Ok(Some(crate::commands::LocalCommand::Log(log))) => {
                                handle_log_command(&shared.action_logs, &account_name, log).await;
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
                shared.action_logs.packet(&account_name, "C2S", frame.opcode, &frame.body).await;
                if frame.opcode == CMSG_WARDEN_DATA::OPCODE { warden.client_to_server(&mut frame.body); }
                if let Err(e)=write_client_frame(&mut uw, &mut up_enc, &frame).await { break Err(e); }
            }
            command = commands.recv() => {
                match command {
                    Ok(command) => {
                        {
                        if let GameplayCommand::Loot(target) = &command {
                            bot_loot_target = Some(*target);
                        }
                        if let Some((spell, target)) = bot_targeted_cast(&command) {
                            last_bot_cast = Some((spell, target, std::time::Instant::now()));
                        }
                        let gameobject_report_use = match &command {
                            GameplayCommand::CastGameObject { target, report_use: true, .. } => Some(*target),
                            _ => None,
                        };
                        let movement_context = gameplay_movement_context(
                            controlled_mover,
                            controlled_position,
                            controlled_flags,
                            player_guid,
                            canonical_position,
                            canonical_flags,
                        );
                        match encode_gameplay_command(
                            command,
                            movement_context.mover,
                            movement_context.position,
                            movement_context.flags,
                            &mut movement_clock,
                        ) {
                            Ok(Some((frame, movement))) => {
                                tracing::info!(account=%account_name, opcode=frame.opcode, "bot gameplay packet transmitted");
                                if let Err(e)=write_client_frame(&mut uw, &mut up_enc, &frame).await { break Err(e); }
                                if let Some(target) = gameobject_report_use {
                                    let report = ClientFrame { opcode: 0x0481, body: target.0.to_le_bytes().to_vec() };
                                    tracing::info!(account=%account_name, opcode=report.opcode, ?target, "bot game-object report-use packet transmitted");
                                    if let Err(e)=write_client_frame(&mut uw, &mut up_enc, &report).await { break Err(e); }
                                }
                                if let Some((position, moving, flags, client_time)) = movement {
                                    if controlled_mover.is_some() {
                                        controlled_position = Some(position);
                                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::ControlledMover { mover: controlled_mover, position: Some(position), flags })).await;
                                    } else {
                                        canonical_position = Some(position);
                                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::PlayerPosition { position, moving, flags, client_time })).await;
                                    }
                                    last_bot_visual = Some((position, frame.opcode, std::time::Instant::now()));
                                    if let Ok(opcode) = u16::try_from(frame.opcode) {
                                        let visual = crate::framing::ServerFrame { opcode, body: frame.body.clone() };
                                        if let Err(error) = write_server_frame(&mut dw, &mut down_enc, &visual).await {
                                            break Err(error);
                                        }
                                    }
                                }
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
            server = read_server_frame(&mut ur, &mut up_dec) => {
                let mut frame = match server { Ok(v)=>v, Err(e)=>break Err(e) };
                shared.action_logs.packet(&account_name, "S2C", u32::from(frame.opcode), &frame.body).await;
                const SMSG_LOOT_RESPONSE_OPCODE: u32 = 0x0160;
                const SMSG_LOOT_RELEASE_RESPONSE_OPCODE: u32 = 0x0161;
                const SMSG_LOOT_REMOVED_OPCODE: u32 = 0x0162;
                const SMSG_LOOT_CLEAR_MONEY_OPCODE: u32 = 0x0163;
                const CMSG_AUTOSTORE_LOOT_ITEM_OPCODE: u32 = 0x0108;
                const CMSG_LOOT_MONEY_OPCODE: u32 = 0x015E;
                const CMSG_LOOT_RELEASE_OPCODE: u32 = 0x015F;
                let mut suppress_bot_loot_frame = false;
                if u32::from(frame.opcode) == SMSG_LOOT_RESPONSE_OPCODE {
                    if let Some((guid, gold, slots)) = parse_loot_response(&frame.body) {
                        if bot_loot_target.is_some_and(|target| target.0 == guid) {
                            suppress_bot_loot_frame = true;
                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootOpened { target: EntityId(guid), ownership: wow_state::observation::LootOwnership::Bot })).await;
                            tracing::info!(account=%account_name, loot_guid=guid, gold, slots=?slots, "bot-owned loot window opened; collecting slots");
                            if gold > 0 {
                                let money = ClientFrame { opcode: CMSG_LOOT_MONEY_OPCODE, body: Vec::new() };
                                shared.action_logs.packet(&account_name, "C2S", money.opcode, &money.body).await;
                                if let Err(e) = write_client_frame(&mut uw, &mut up_enc, &money).await { break Err(e); }
                            }
                            let mut loot_slot_write_error = None;
                            for slot in slots {
                                let take = ClientFrame { opcode: CMSG_AUTOSTORE_LOOT_ITEM_OPCODE, body: vec![slot] };
                                shared.action_logs.packet(&account_name, "C2S", take.opcode, &take.body).await;
                                if let Err(error) = write_client_frame(&mut uw, &mut up_enc, &take).await {
                                    loot_slot_write_error = Some(error);
                                    break;
                                }
                            }
                            if let Some(error) = loot_slot_write_error {
                                break Err(error);
                            }
                            let release = ClientFrame { opcode: CMSG_LOOT_RELEASE_OPCODE, body: guid.to_le_bytes().to_vec() };
                            shared.action_logs.packet(&account_name, "C2S", release.opcode, &release.body).await;
                            if let Err(e) = write_client_frame(&mut uw, &mut up_enc, &release).await { break Err(e); }
                        }
                    }
                } else if matches!(u32::from(frame.opcode), SMSG_LOOT_RELEASE_RESPONSE_OPCODE | SMSG_LOOT_REMOVED_OPCODE | SMSG_LOOT_CLEAR_MONEY_OPCODE) && bot_loot_target.is_some() {
                    suppress_bot_loot_frame = true;
                    if u32::from(frame.opcode) == SMSG_LOOT_RELEASE_RESPONSE_OPCODE {
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootClosed { ownership: wow_state::observation::LootOwnership::Bot })).await;
                        tracing::info!(account=%account_name, "bot-owned loot transaction released");
                        bot_loot_target = None;
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
                                    object_observer.set_world(position.map, Some(guid));
                                    canonical_position = Some(*position);
                                    canonical_flags = 0;
                                    let _ = account.command_bus.send(GameplayCommand::StopMovement);
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
                        for observation in observations {
                            if let ProtocolObservation::ControlledMover { mover, position, flags } = &observation {
                                controlled_mover = *mover;
                                controlled_position = *position;
                                controlled_flags = *flags;
                                tracing::info!(account=%account_name, ?controlled_mover, has_position=controlled_position.is_some(), "authoritative controlled mover changed");
                            }
                            let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                        }
                    }
                    Err(error) => tracing::warn!(account=%account_name, opcode=frame.opcode, %error, "Tentacli object observation failed; packet still relayed to client"),
                }
                if u32::from(frame.opcode) == SMSG_WARDEN_DATA::OPCODE { warden.server_to_client(&mut frame.body); }
                if !suppress_bot_loot_frame {
                    if let Err(e)=write_server_frame(&mut dw, &mut down_enc, &frame).await { break Err(e); }
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
    let mut last_bot_cast: Option<(u32, Option<EntityId>, std::time::Instant)> = None;
    let mut char_enum_requested = false;
    let mut player_login_requested = false;
    let mut world_transfer_pending = false;
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
                    Ok(command) => {
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
                                if let Some((spell, target)) = bot_targeted_cast(&command) {
                                    last_bot_cast = Some((spell, target, std::time::Instant::now()));
                                }
                                if matches!(command, GameplayCommand::Loot(_)) {
                                    if let GameplayCommand::Loot(target) = command { bot_loot_target = Some(target); }
                                }
                                write_client_frame(&mut writer, &mut enc, &frame).await?;
                                tracing::info!(account=%account.config.account_name, opcode=frame.opcode, "headless bot gameplay packet transmitted");
                                if let Some((position, moving, flags, client_time)) = movement {
                                    if controlled_mover.is_some() {
                                        controlled_position = Some(position);
                                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::ControlledMover { mover: controlled_mover, position: Some(position), flags })).await;
                                    } else {
                                        canonical_position = Some(position);
                                        canonical_flags = flags;
                                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::PlayerPosition { position, moving, flags, client_time })).await;
                                    }
                                }
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
                write_client_frame(&mut writer, &mut enc, &frame).await?;
                pending_ping = Some((sequence, tokio::time::Instant::now()));
                tracing::debug!(account=%account.config.account_name, sequence, opcode=frame.opcode, "headless client keepalive ping transmitted");
                next_ping = tokio::time::Instant::now() + ping_interval;
            }
            server = server_rx.recv() => {
                let frame = server.context("headless upstream reader ended")??;
                let opcode = u32::from(frame.opcode);
                if opcode == 0x003F {
                    world_transfer_pending = true;
                    tracing::info!(account=%account.config.account_name, "headless world transfer started");
                }
                if frame.opcode == SMSG_TIME_SYNC_REQ_OPCODE {
                    let counter = parse_time_sync_request(&frame.body)
                        .context("SMSG_TIME_SYNC_REQ is missing its u32 counter")?;
                    let response = encode_time_sync_response(
                        counter,
                        movement_clock.current_timestamp(),
                    );
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
                    write_client_frame(&mut writer, &mut enc, &ClientFrame {
                        opcode: 0x00C7,
                        body,
                    }).await?;
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
                    write_client_frame(&mut writer, &mut enc, &ClientFrame { opcode: 0x0037, body: Vec::new() }).await?;
                    char_enum_requested = true;
                    tracing::info!(account=%account.config.account_name, "headless character enumeration requested");
                    continue;
                }
                if opcode == 0x003B && !player_login_requested {
                    let (guid, name) = select_wrath_character(&frame.body, account.config.character.as_deref())
                        .context("configured headless character was not present in SMSG_CHAR_ENUM")?;
                    player_guid = Some(EntityId(guid));
                    object_observer.set_player_guid(EntityId(guid));
                    write_client_frame(&mut writer, &mut enc, &ClientFrame { opcode: 0x003D, body: guid.to_le_bytes().to_vec() }).await?;
                    player_login_requested = true;
                    tracing::info!(account=%account.config.account_name, character=%name, guid=%format_args!("0x{guid:016X}"), "headless configured character login requested");
                    continue;
                }
                if opcode == u32::from(SMSG_WARDEN_DATA::OPCODE) {
                    let reply = warden.handle(&frame.body).context("headless Warden exchange failed")?;
                    if let Some(body) = reply.body {
                        write_client_frame(&mut writer, &mut enc, &ClientFrame {
                            opcode: CMSG_WARDEN_DATA::OPCODE,
                            body,
                        }).await?;
                    }
                    if reply.event != "module chunk" {
                        tracing::info!(account=%account.config.account_name, event=reply.event, "headless Warden exchange advanced");
                    }
                    continue;
                }
                if opcode == 0x0160 {
                    if let Some((guid, gold, slots)) = parse_loot_response(&frame.body) {
                        if bot_loot_target.is_some_and(|target| target.0 == guid) {
                            let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootOpened { target: EntityId(guid), ownership: wow_state::observation::LootOwnership::Bot })).await;
                            if gold > 0 { write_client_frame(&mut writer, &mut enc, &ClientFrame { opcode: 0x015E, body: Vec::new() }).await?; }
                            for slot in slots { write_client_frame(&mut writer, &mut enc, &ClientFrame { opcode: 0x0108, body: vec![slot] }).await?; }
                            write_client_frame(&mut writer, &mut enc, &ClientFrame { opcode: 0x015F, body: guid.to_le_bytes().to_vec() }).await?;
                        }
                    }
                } else if opcode == 0x0161 && bot_loot_target.is_some() {
                    let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::LootClosed { ownership: wow_state::observation::LootOwnership::Bot })).await;
                    bot_loot_target = None;
                }
                if opcode == 0x0130 {
                    if let Some((_cast_count, spell, reason, target)) =
                        take_bot_cast_failure(&frame.body, &mut last_bot_cast)
                    {
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::CastFailed { spell, reason, target })).await;
                    }
                }
                if opcode == 0x003E {
                    if let Some(guid) = player_guid
                        && let Some(ProtocolObservation::EnteredWorld { position: Some(position), .. }) = login_verify_world(&frame.body, guid.0)
                    {
                        canonical_position = Some(position);
                        canonical_flags = 0;
                        controlled_mover = None;
                        controlled_position = None;
                        controlled_flags = 0;
                        object_observer.set_world(position.map, Some(guid));
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::EnteredWorld {
                            character_guid: guid.0,
                            position: Some(position),
                        })).await;
                    }
                    // Keep movement_clock alive across the transfer. The
                    // server's clockDelta still maps this client clock to its
                    // clock for the lifetime of this world session.
                    // MSG_MOVE_WORLDPORT_ACK has an empty body. The server holds
                    // a far teleport open until this packet arrives.
                    write_client_frame(&mut writer, &mut enc, &ClientFrame {
                        opcode: 0x00DC,
                        body: Vec::new(),
                    }).await?;
                    world_transfer_pending = false;
                    tracing::info!(account=%account.config.account_name, opcode, "headless world transfer acknowledged");
                }
                if opcode == 0x0236 {
                    if let Some(guid) = player_guid {
                        if let Some(observation) = login_verify_world(&frame.body, guid.0) {
                            if let ProtocolObservation::EnteredWorld { position: Some(position), .. } = &observation {
                                object_observer.set_world(position.map, Some(guid));
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
                            let _ = account.session_tx.send(SessionMessage::BotOn).await;
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
        GameplayCommand::MaintainBuff { spell, target } => Some((*spell, Some(*target))),
        GameplayCommand::CastGameObject { spell, target, .. } => Some((*spell, Some(*target))),
        GameplayCommand::UseItemInstance { spell, target, .. } if *spell != 0 => {
            Some((*spell, *target))
        }
        _ => None,
    }
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

async fn handle_log_command(
    logs: &ActionLogManager,
    account: &str,
    command: crate::commands::log::LogCommand,
) {
    use crate::commands::log::LogCommand;
    match command {
        LogCommand::Start => match logs.start(account).await {
            Ok(path) => {
                tracing::info!(account=%account, path=%path.display(), "action logging started")
            }
            Err(error) => {
                tracing::error!(account=%account, %error, "failed to start action logging")
            }
        },
        LogCommand::Stop => match logs.stop(account).await {
            Ok(Some(path)) => {
                tracing::info!(account=%account, path=%path.display(), "action logging stopped")
            }
            Ok(None) => tracing::info!(account=%account, "action logging is not active"),
            Err(error) => {
                tracing::error!(account=%account, %error, "failed to stop action logging")
            }
        },
        LogCommand::Status => match logs.status(account).await {
            Some(path) => {
                tracing::info!(account=%account, path=%path.display(), "action logging is active")
            }
            None => tracing::info!(account=%account, "action logging is inactive"),
        },
        LogCommand::Mark(label) => match logs.mark(account, label.as_deref()).await {
            Ok(true) => tracing::info!(account=%account, ?label, "action log marker written"),
            Ok(false) => {
                tracing::info!(account=%account, "action logging is not active; marker ignored")
            }
            Err(error) => {
                tracing::error!(account=%account, %error, "failed to write action log marker")
            }
        },
    }
}

fn port_of(bind: &str) -> Result<u16> {
    Ok(bind.parse::<SocketAddr>()?.port())
}
fn advertised(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod runtime_chat_tests {
    use super::*;
    use crate::commands::{self, LocalCommand};

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
            0x0DA,
            visual.point,
            1.25,
            0x0DA
        ));
        assert!(!movement_matches_bot_visual(
            visual,
            0x0DA,
            Vec3::new(2.0, 2.0, 3.0),
            1.25,
            0x0DA
        ));
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
        assert!(matches!(
            parse_aura_update(&aura),
            Some(ProtocolObservation::AuraSlot {
                entity: EntityId(7),
                slot: 3,
                aura: Some(wow_state::auras::AuraInstance { spell: 1459, .. })
            })
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
        let (parsed_guid, gold, slots) =
            parse_loot_response(&body).expect("loot response should parse");
        assert_eq!(parsed_guid, guid);
        assert_eq!(gold, 7);
        assert_eq!(slots, vec![1, 2, 3]);
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
            let (guid, gold, slots) =
                parse_loot_response(&body).expect("captured Wrath loot response should parse");
            assert_eq!(guid, expected_guid);
            assert_eq!(gold, 0);
            assert_eq!(slots, expected_slots);
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
        let parsed = parse_quest_offer_reward(&offer).expect("offer-reward should parse");
        assert!(matches!(
            parsed,
            ProtocolObservation::QuestTurnInDialog {
                quest: 42,
                dialog: wow_state::quests::QuestTurnInDialog {
                    giver: EntityId(77),
                    stage: wow_state::quests::QuestTurnInStage::OfferReward { reward_choices: 2 }
                }
            }
        ));
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
}
