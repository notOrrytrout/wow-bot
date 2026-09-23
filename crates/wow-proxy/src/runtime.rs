use crate::{
    auth::{
        limits::{AdmissionController, AdmissionLimits},
        login::{ConfiguredCredential, UpstreamAuthConfig, upstream_login},
    },
    configured_session::{ConfiguredSessionActor, SessionMessage, state::ConfiguredSessionState},
    framing::{ClientFrame, client_edge::{read_client_frame, write_client_frame}, upstream_edge::{read_server_frame, write_server_frame}},
    transparent_session::relay::relay as transparent_relay,
    warden::{client::WardenClient, relay::WardenBridge},
};
use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::Write as _,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, watch, RwLock},
};
use wow_control_proto::{ProxyToWorker, SupervisorCommand, WorkerToProxy};
use wow_domain::{AccountId, EntityId, GameplayCommand, LaneId, Mission, MissionId, Vec3, WorldPosition, WorkerGeneration};
use wow_state::ProtocolObservation;
use wow_tentacli_adapter::runtime::{login_verify_world, ObjectObservationRuntime};
use wow_login_messages::{
    all::ProtocolVersion,
    helper::{tokio_expect_client_message, tokio_expect_client_message_protocol, tokio_expect_server_message_protocol},
    version_8::{
        CMD_REALM_LIST_Client, CMD_REALM_LIST_Server, Realm, RealmCategory, RealmType,
    },
};
use wow_srp::{normalized_string::NormalizedString, wrath_header::ProofSeed};
use wow_login_messages::Message as _;
use wow_world_messages::Message as _;
use wow_world_messages::wrath::{
    ClientMessage as _, ServerMessage as _, CMSG_AUTH_SESSION, CMSG_WARDEN_DATA, SMSG_AUTH_CHALLENGE, SMSG_WARDEN_DATA,
    tokio_expect_client_message as world_expect_client_message,
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
        let mut state = worlds.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.next_id = state.next_id.checked_add(1).expect("player connection IDs exhausted");
        let connection = state.next_id;
        state.active.insert(connection);
        let _ = headless_enabled.send(false);
        drop(state);
        Self { connection, worlds, headless_enabled }
    }
}

impl Drop for PlayerWorldHandoff {
    fn drop(&mut self) {
        let mut state = self.worlds.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    async fn put(&self, account: &str, key: [u8; 40]) { self.keys.write().await.insert(account.to_uppercase(), key); }
    async fn get(&self, account: &str) -> Option<[u8; 40]> { self.keys.read().await.get(&account.to_uppercase()).copied() }
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
        Self { dir, sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())) }
    }

    async fn start(&self, account: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let safe = account.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>();
        let path = self.dir.join(format!("action-{safe}-{stamp}.jsonl"));
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{{\"event\":\"start\",\"account\":{:?},\"unix_s\":{stamp}}}", account)?;
        file.flush()?;
        self.sessions.lock().await.insert(account.to_uppercase(), ActionLogSession { path: path.clone(), file });
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
        self.sessions.lock().await.get(&account.to_uppercase()).map(|s| s.path.clone())
    }

    async fn mark(&self, account: &str, label: Option<&str>) -> Result<bool> {
        let mut sessions = self.sessions.lock().await;
        let Some(session) = sessions.get_mut(&account.to_uppercase()) else { return Ok(false) };
        let label = label.unwrap_or("mark").replace('"', "'");
        writeln!(session.file, "{{\"event\":\"mark\",\"label\":{:?}}}", label)?;
        session.file.flush()?;
        Ok(true)
    }

    async fn packet(&self, account: &str, direction: &str, opcode: u32, body: &[u8]) {
        let mut sessions = self.sessions.lock().await;
        let Some(session) = sessions.get_mut(&account.to_uppercase()) else { return };
        let fingerprint = body.iter().fold(0xcbf29ce484222325_u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3));
        let _ = writeln!(session.file, "{{\"event\":\"packet\",\"direction\":{:?},\"opcode\":{},\"body_len\":{},\"fingerprint\":{:?}}}", direction, opcode, body.len(), format!("{fingerprint:016X}"));
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
    let mut accounts = HashMap::new();
    for lane in lanes {
        let (session_tx, session_rx) = mpsc::channel(256);
        let (command_tx, mut command_rx) = mpsc::channel(256);
        let (command_bus, _) = broadcast::channel(256);
        let (headless_enabled, _) = watch::channel(true);
        let headless_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let state = ConfiguredSessionState::new(lane.config.account, lane.config.lane, lane.config.worker);
        let supervisor_for_runtime = lane.supervisor_tx.clone();
        tokio::spawn(ConfiguredSessionActor {
            state,
            rx: session_rx,
            worker_tx: lane.worker_tx,
            upstream_tx: command_tx.clone(),
            supervisor_tx: lane.supervisor_tx,
        }.run());
        let session = session_tx.clone();
        tokio::spawn(async move {
            let mut worker_rx = lane.worker_rx;
            while let Some(message) = worker_rx.recv().await {
                if session.send(SessionMessage::Worker(message)).await.is_err() { break; }
            }
        });
        let bus = command_bus.clone();
        tokio::spawn(async move { while let Some(command) = command_rx.recv().await { let _ = bus.send(command); } });
        accounts.insert(lane.config.account_name.to_uppercase(), AccountRuntime { config: lane.config, session_tx, command_bus, supervisor_tx: supervisor_for_runtime, mission_counter: Arc::new(AtomicU64::new(1)), headless_enabled, headless_active, player_worlds: Arc::new(Mutex::new(PlayerWorlds::default())) });
    }
    let action_logs = ActionLogManager::new(config.log_dir.clone());
    let shared = SharedRuntime { config: Arc::new(config), accounts: Arc::new(accounts), auth: AuthRegistry::default(), action_logs };
    for account in shared.accounts.values().cloned() {
        let shared_for_headless = shared.clone();
        tokio::spawn(async move { headless_session_manager(shared_for_headless, account).await; });
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
    for account in shared.accounts.values() { let _ = account.session_tx.send(SessionMessage::Shutdown).await; }
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
            Err(reason) => { tracing::warn!(%peer, reason, "rejected pre-auth connection"); continue; }
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let result = tokio::time::timeout(shared.config.handshake_timeout, handle_auth(shared.clone(), stream)).await;
            drop(permit);
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(%peer, error=%format_args!("{e:#}"), "auth connection failed"),
                Err(_) => tracing::warn!(%peer, "auth handshake timed out"),
            }
        });
    }
}

struct StockChallenge { account: String, raw: Vec<u8> }
async fn read_stock_challenge(stream: &mut TcpStream) -> Result<StockChallenge> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await?;
    if prefix[0] != 0 || prefix[1] != 8 { bail!("expected WotLK 3.3.5a login challenge"); }
    let size = usize::from(u16::from_le_bytes([prefix[2], prefix[3]]));
    if !(30..=4096).contains(&size) { bail!("invalid login challenge length {size}"); }
    let mut body = vec![0_u8; size]; stream.read_exact(&mut body).await?;
    let n = usize::from(*body.get(29).context("missing account length")?);
    let account = std::str::from_utf8(body.get(30..30+n).context("truncated account name")?)?.to_owned();
    let mut raw = prefix.to_vec(); raw.extend_from_slice(&body);
    Ok(StockChallenge { account, raw })
}

async fn handle_auth(shared: SharedRuntime, mut downstream: TcpStream) -> Result<()> {
    let challenge = read_stock_challenge(&mut downstream).await?;
    let key = challenge.account.to_uppercase();
    if let Some(account) = shared.accounts.get(&key) {
        let upstream = upstream_login(&upstream_config(&shared, account), &account.config.password).await?;
        let authenticated = crate::auth::login::terminate_configured_downstream(&mut downstream, &ConfiguredCredential {
            account: account.config.account_name.clone(), password: account.config.password.clone()
        }).await?;
        shared.auth.put(&key, authenticated.session_key).await;
        serve_configured_realms(&shared, &mut downstream, &upstream.realm_name).await
    } else {
        transparent_auth(shared, challenge, downstream).await
    }
}

fn upstream_config(shared: &SharedRuntime, account: &AccountRuntime) -> UpstreamAuthConfig {
    UpstreamAuthConfig {
        host: shared.config.upstream_auth_host.clone(), port: shared.config.upstream_auth_port,
        realm_name: shared.config.realm_name.clone(), world_host: shared.config.upstream_world_host.clone(),
        world_port: shared.config.upstream_world_port, account: account.config.account_name.clone(),
    }
}

async fn serve_configured_realms(shared: &SharedRuntime, stream: &mut TcpStream, realm_name: &str) -> Result<()> {
    let address = advertised(&shared.config.advertise_host, port_of(&shared.config.world_bind)?);
    while tokio_expect_client_message::<CMD_REALM_LIST_Client, _>(&mut *stream).await.is_ok() {
        CMD_REALM_LIST_Server { realms: vec![Realm {
            realm_type: RealmType::PlayerVsEnvironment, locked: false, flag: Default::default(),
            name: realm_name.to_owned(), address: address.clone(), population: Default::default(),
            number_of_characters_on_realm: 1, category: RealmCategory::One, realm_id: 1,
        }]}.tokio_write(&mut *stream).await?;
    }
    Ok(())
}

async fn transparent_auth(shared: SharedRuntime, challenge: StockChallenge, mut downstream: TcpStream) -> Result<()> {
    let addr = advertised(&shared.config.upstream_auth_host, shared.config.upstream_auth_port);
    let mut upstream = TcpStream::connect(&addr).await?;
    upstream.write_all(&challenge.raw).await?;
    // Relay challenge and proof without deriving the unknown account session key.
    let response = tokio_expect_server_message_protocol::<wow_login_messages::version_8::CMD_AUTH_LOGON_CHALLENGE_Server, _>(&mut upstream, ProtocolVersion::Eight).await?;
    response.tokio_write(&mut downstream).await?;
    let proof = tokio_expect_client_message_protocol::<wow_login_messages::version_8::CMD_AUTH_LOGON_PROOF_Client, _>(&mut downstream, ProtocolVersion::Eight).await?;
    proof.tokio_write(&mut upstream).await?;
    let response = tokio_expect_server_message_protocol::<wow_login_messages::version_8::CMD_AUTH_LOGON_PROOF_Server, _>(&mut upstream, ProtocolVersion::Eight).await?;
    response.tokio_write(&mut downstream).await?;
    loop {
        let request = match tokio_expect_client_message_protocol::<CMD_REALM_LIST_Client, _>(&mut downstream, ProtocolVersion::Eight).await { Ok(v)=>v, Err(_)=>break };
        request.tokio_write(&mut upstream).await?;
        let mut realms = tokio_expect_server_message_protocol::<CMD_REALM_LIST_Server, _>(&mut upstream, ProtocolVersion::Eight).await?;
        for realm in &mut realms.realms {
            if realm.name.eq_ignore_ascii_case(&shared.config.realm_name) {
                realm.address = advertised(&shared.config.advertise_host, port_of(&shared.config.transparent_world_bind)?);
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
        let addr = advertised(&shared.config.upstream_world_host, shared.config.upstream_world_port);
        tokio::spawn(async move {
            match TcpStream::connect(&addr).await {
                Ok(mut upstream) => match transparent_relay(&mut downstream, &mut upstream).await {
                    Ok((up, down)) => tracing::debug!(%peer, up, down, "transparent world relay ended"),
                    Err(e) => tracing::warn!(%peer, %e, "transparent world relay failed"),
                },
                Err(e) => tracing::warn!(%peer, %e, upstream=%addr, "transparent world connect failed"),
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
    SMSG_AUTH_CHALLENGE { unknown1: 1, server_seed: seed.seed(), seed: [0_u8; 32] }
        .tokio_write_unencrypted_server(&mut downstream).await?;
    let auth = world_expect_client_message::<CMSG_AUTH_SESSION, _>(&mut downstream).await?;
    let account_name = auth.username.to_uppercase();
    let account = shared.accounts.get(&account_name).with_context(|| format!("world account {account_name} is not configured"))?.clone();
    let handoff = PlayerWorldHandoff::begin(account.player_worlds.clone(), account.headless_enabled.clone());
    wait_for_headless_state(&account, false, Duration::from_secs(6)).await?;
    let downstream_key = shared.auth.get(&account_name).await.context("no fresh downstream login session key for configured world connection")?;
    let normalized = NormalizedString::new(&account_name)?;
    let downstream_crypto = seed.into_server_header_crypto(&normalized, downstream_key, auth.client_proof, auth.client_seed)?;
    let (mut down_enc, mut down_dec) = downstream_crypto.split();

    let upstream_login = upstream_login(&upstream_config(&shared, &account), &account.config.password).await?;
    let addr = advertised(&upstream_login.world_host, upstream_login.world_port);
    let mut upstream = TcpStream::connect(&addr).await.with_context(|| format!("connect upstream world {addr}"))?;
    let challenge = world_expect_server_message::<SMSG_AUTH_CHALLENGE, _>(&mut upstream).await?;
    let up_seed = ProofSeed::new();
    let client_seed = up_seed.seed();
    let (proof, up_crypto) = up_seed.into_client_header_crypto(&normalized, upstream_login.session_key, challenge.server_seed);
    CMSG_AUTH_SESSION {
        client_build: 12340, login_server_id: 0, username: account_name.clone(), login_server_type: 0,
        client_seed, region_id: 0, battleground_id: 0, realm_id: upstream_login.realm_id,
        dos_response: 0, client_proof: proof, addon_info: auth.addon_info,
    }.tokio_write_unencrypted_client(&mut upstream).await?;
    let (mut up_enc, mut up_dec) = up_crypto.split();
    let mut warden = WardenBridge::new(&upstream_login.session_key, &downstream_key);

    let connection = handoff.connection;
    let mut commands = account.command_bus.subscribe();
    account.session_tx.send(SessionMessage::PlayerAttached { connection }).await.ok();
    account.session_tx.send(SessionMessage::UpstreamConnected(true)).await.ok();
    tracing::info!(account=%account_name, upstream=%addr, "configured player world bridge attached");

    let mut object_observer = ObjectObservationRuntime::new().context("initialize Tentacli object observer")?;
    let mut player_guid: Option<EntityId> = None;
    let mut canonical_position: Option<WorldPosition> = None;
    let mut controlled_mover: Option<EntityId> = None;
    let mut controlled_position: Option<WorldPosition> = None;
    let mut controlled_flags: u32 = 0;
    let mut canonical_flags: u32 = 0;
    let mut movement_time: u32 = 1;
    let mut last_bot_visual: Option<(WorldPosition, u32, std::time::Instant)> = None;
    let mut bot_loot_target: Option<EntityId> = None;
    let mut last_bot_cast: Option<(u32, Option<EntityId>, std::time::Instant)> = None;
    let (mut dr, mut dw) = downstream.into_split();
    let (mut ur, mut uw) = upstream.into_split();
    let result: Result<()> = loop {
        tokio::select! {
            client = read_client_frame(&mut dr, &mut down_dec) => {
                let mut frame = match client { Ok(v)=>v, Err(e)=>break Err(e) };
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
                                    movement_time = client_time;
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
                        let mover_guid = controlled_mover.or(player_guid);
                        let mover_position = controlled_position.or(canonical_position);
                        let mover_flags = if controlled_mover.is_some() { controlled_flags } else { canonical_flags };
                        match encode_gameplay_command(command, mover_guid, mover_position, mover_flags, &mut movement_time) {
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
                    if let Some((cast_count, spell, reason)) = parse_cast_failed(&frame.body) {
                        let target = last_bot_cast
                            .take()
                            .filter(|(pending_spell, _, at)| *pending_spell == spell && at.elapsed() < Duration::from_secs(5))
                            .and_then(|(_, target, _)| target);
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
                for observation in quest_observations(u32::from(frame.opcode), &frame.body) {
                    tracing::debug!(account=%account_name, ?observation, "authoritative quest observation");
                    let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                }
                for observation in maintenance_observations(u32::from(frame.opcode), &frame.body) {
                    tracing::debug!(account=%account_name, ?observation, "authoritative maintenance observation");
                    let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                }
                if let Some(observation) = controlled_abilities_observation(u32::from(frame.opcode), &frame.body) {
                    tracing::info!(account=%account_name, ?observation, "authoritative controlled-unit abilities observed");
                    let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                }
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
    account.session_tx.send(SessionMessage::PlayerDetached { connection }).await.ok();
    result
}


async fn wait_for_headless_state(account: &AccountRuntime, expected: bool, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if account.headless_active.load(Ordering::Acquire) == expected { return Ok(()); }
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
            if enabled.changed().await.is_err() { return; }
        }
        tracing::info!(account=%account.config.account_name, ?account.config.character, "headless autonomous session requested");
        account.headless_active.store(true, Ordering::Release);
        let result = run_headless_world_session(shared.clone(), account.clone(), &mut enabled).await;
        account.headless_active.store(false, Ordering::Release);
        let _ = account.session_tx.send(SessionMessage::WorldAuthoritative(false)).await;
        let _ = account.session_tx.send(SessionMessage::UpstreamConnected(false)).await;
        if !*enabled.borrow() {
            tracing::info!(account=%account.config.account_name, "headless autonomous session yielded to attended player");
            retry = Duration::from_secs(1);
            continue;
        }
        match result {
            Ok(()) => tracing::info!(account=%account.config.account_name, "headless autonomous session ended; reconnect scheduled"),
            Err(error) => tracing::warn!(account=%account.config.account_name, error=%format_args!("{error:#}"), "headless autonomous session failed; reconnect scheduled"),
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
    if !*enabled.borrow() { return Ok(()); }
    let upstream_login = upstream_login(&upstream_config(&shared, &account), &account.config.password).await?;
    let addr = advertised(&upstream_login.world_host, upstream_login.world_port);
    let mut upstream = TcpStream::connect(&addr).await.with_context(|| format!("connect headless upstream world {addr}"))?;
    let challenge = world_expect_server_message::<SMSG_AUTH_CHALLENGE, _>(&mut upstream).await?;
    let normalized = NormalizedString::new(&account.config.account_name.to_uppercase())?;
    let seed = ProofSeed::new();
    let client_seed = seed.seed();
    let (proof, crypto) = seed.into_client_header_crypto(&normalized, upstream_login.session_key, challenge.server_seed);
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
    }.tokio_write_unencrypted_client(&mut upstream).await?;
    let (mut enc, mut dec) = crypto.split();
    let (mut reader, mut writer) = upstream.into_split();
    let mut warden = WardenClient::new(
        &upstream_login.session_key,
        shared.config.warden_client_image.as_deref(),
    )?;
    let mut commands = account.command_bus.subscribe();
    let mut object_observer = ObjectObservationRuntime::new().context("initialize headless Tentacli object observer")?;
    let mut player_guid = None;
    let mut canonical_position = None;
    let mut canonical_flags = 0_u32;
    let mut controlled_mover = None;
    let mut controlled_position = None;
    let mut controlled_flags = 0_u32;
    let mut movement_time = 1_u32;
    let mut bot_loot_target = None;
    let mut last_bot_cast: Option<(u32, Option<EntityId>, std::time::Instant)> = None;
    let mut char_enum_requested = false;
    let mut player_login_requested = false;

    account.session_tx.send(SessionMessage::UpstreamConnected(true)).await.ok();
    tracing::info!(account=%account.config.account_name, upstream=%addr, "headless upstream world authenticated socket established");

    loop {
        tokio::select! {
            changed = enabled.changed() => {
                if changed.is_err() || !*enabled.borrow() { return Ok(()); }
            }
            command = commands.recv() => {
                match command {
                    Ok(command) => {
                        let mover_guid = controlled_mover.or(player_guid);
                        let mover_position = controlled_position.or(canonical_position);
                        let mover_flags = if controlled_mover.is_some() { controlled_flags } else { canonical_flags };
                        match encode_gameplay_command(command.clone(), mover_guid, mover_position, mover_flags, &mut movement_time) {
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
            server = read_server_frame(&mut reader, &mut dec) => {
                let frame = server?;
                let opcode = u32::from(frame.opcode);
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
                    if let Some((_cast_count, spell, reason)) = parse_cast_failed(&frame.body) {
                        let target = last_bot_cast.take().filter(|(pending, _, at)| *pending == spell && at.elapsed() < Duration::from_secs(5)).and_then(|(_, target, _)| target);
                        let _ = account.session_tx.send(SessionMessage::Observation(ProtocolObservation::CastFailed { spell, reason, target })).await;
                    }
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
                for observation in quest_observations(opcode, &frame.body) {
                    let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                }
                for observation in maintenance_observations(opcode, &frame.body) {
                    let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                }
                if let Some(observation) = controlled_abilities_observation(opcode, &frame.body) {
                    let _ = account.session_tx.send(SessionMessage::Observation(observation)).await;
                }
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
        if first.is_none() { first = Some((guid, name.clone())); }
        if configured.is_some_and(|wanted| name.eq_ignore_ascii_case(wanted)) { return Some((guid, name)); }
        rest = rest.get(FIXED_TAIL_AFTER_NAME..)?;
    }
    if configured.is_none_or(|name| name.trim().is_empty()) { first } else { None }
}

fn bot_targeted_cast(command: &GameplayCommand) -> Option<(u32, Option<EntityId>)> {
    match command {
        GameplayCommand::Cast { spell, target } | GameplayCommand::VehicleCast { spell, target } => Some((*spell, *target)),
        GameplayCommand::MaintainBuff { spell, target } => Some((*spell, Some(*target))),
        GameplayCommand::CastGameObject { spell, target, .. } => Some((*spell, Some(*target))),
        GameplayCommand::UseItemInstance { spell, target, .. } if *spell != 0 => Some((*spell, *target)),
        _ => None,
    }
}

fn parse_cast_failed(body: &[u8]) -> Option<(u8, u32, u8)> {
    // AzerothCore Spell::WriteCastResultInfo: cast_count:u8, spell:u32, reason:u8.
    let cast_count = *body.first()?;
    let spell = u32::from_le_bytes(body.get(1..5)?.try_into().ok()?);
    let reason = *body.get(5)?;
    Some((cast_count, spell, reason))
}

fn parse_loot_response(body: &[u8]) -> Option<(u64, u32, Vec<u8>)> {
    // Wrath SMSG_LOOT_RESPONSE: guid:u64, loot_type:u8, gold:u32,
    // item_count:u8, followed by 22-byte item records beginning with slot:u8.
    let guid = u64::from_le_bytes(body.get(0..8)?.try_into().ok()?);
    let gold = u32::from_le_bytes(body.get(9..13)?.try_into().ok()?);
    let count = usize::from(*body.get(13)?);
    let mut cursor = 14usize;
    let mut slots = Vec::with_capacity(count);
    for _ in 0..count {
        let slot = *body.get(cursor)?;
        body.get(cursor..cursor + 22)?;
        slots.push(slot);
        cursor += 22;
    }
    Some((guid, gold, slots))
}

async fn write_bot_notice<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    encrypter: &mut wow_srp::wrath_header::ServerEncrypterHalf,
    message: &str,
) -> Result<()> {
    const SMSG_MESSAGECHAT_OPCODE: u16 = 0x0096;
    let mut body = Vec::with_capacity(32 + message.len());
    body.push(0); // CHAT_MSG_SYSTEM
    body.extend_from_slice(&0_u32.to_le_bytes()); // LANG_UNIVERSAL
    body.extend_from_slice(&0_u64.to_le_bytes()); // sender GUID
    body.extend_from_slice(&0_u32.to_le_bytes());
    body.extend_from_slice(&0_u64.to_le_bytes()); // target GUID
    body.extend_from_slice(&(u32::try_from(message.len() + 1).unwrap_or(u32::MAX)).to_le_bytes());
    body.extend_from_slice(message.as_bytes());
    body.push(0);
    body.push(0);
    write_server_frame(writer, encrypter, &crate::framing::ServerFrame { opcode: SMSG_MESSAGECHAT_OPCODE, body }).await
}

fn encode_gameplay_command(
    command: GameplayCommand,
    player_guid: Option<EntityId>,
    current: Option<WorldPosition>,
    base_movement_flags: u32,
    movement_time: &mut u32,
) -> std::result::Result<Option<(ClientFrame, Option<(WorldPosition, bool, u32, u32)>)>, String> {
    const CMSG_USE_ITEM: u32 = 0x00AB;
    const CMSG_GAMEOBJ_USE: u32 = 0x00B1;
    const MSG_MOVE_STOP: u32 = 0x00B7;
    const MSG_MOVE_SET_FACING: u32 = 0x00DA;
    const MSG_MOVE_HEARTBEAT: u32 = 0x00EE;
    const CMSG_ATTACKSWING: u32 = 0x0141;
    const CMSG_LOOT: u32 = 0x015D;
    const CMSG_QUESTGIVER_HELLO: u32 = 0x0184;
    const CMSG_QUESTGIVER_ACCEPT_QUEST: u32 = 0x0189;
    const CMSG_QUESTGIVER_COMPLETE_QUEST: u32 = 0x018A;
    const CMSG_QUESTGIVER_REQUEST_REWARD: u32 = 0x018C;
    const CMSG_QUESTGIVER_CHOOSE_REWARD: u32 = 0x018E;
    const CMSG_QUESTGIVER_STATUS_MULTIPLE_QUERY: u32 = 0x0417;
    const CMSG_QUEST_QUERY: u32 = 0x005C;
    match command {
        GameplayCommand::Raw { opcode, body } => Ok(Some((ClientFrame { opcode, body }, None))),
        GameplayCommand::QueryQuestGivers => Ok(Some((ClientFrame { opcode: CMSG_QUESTGIVER_STATUS_MULTIPLE_QUERY, body: Vec::new() }, None))),
        GameplayCommand::QueryQuest { quest } => Ok(Some((ClientFrame { opcode: CMSG_QUEST_QUERY, body: quest.to_le_bytes().to_vec() }, None))),
        GameplayCommand::Interact(entity) => Ok(Some((ClientFrame { opcode: CMSG_QUESTGIVER_HELLO, body: entity.0.to_le_bytes().to_vec() }, None))),
        GameplayCommand::UseGameObject(entity) => Ok(Some((ClientFrame { opcode: CMSG_GAMEOBJ_USE, body: entity.0.to_le_bytes().to_vec() }, None))),
        GameplayCommand::CastGameObject { spell, target, .. } => {
            const CMSG_CAST_SPELL: u32 = 0x012E;
            const TARGET_FLAG_GAMEOBJECT: u32 = 0x0000_0800;
            let mut body = Vec::with_capacity(24);
            body.push(0);
            body.extend_from_slice(&spell.to_le_bytes());
            body.push(0);
            body.extend_from_slice(&TARGET_FLAG_GAMEOBJECT.to_le_bytes());
            push_packed_guid(&mut body, target);
            Ok(Some((ClientFrame { opcode: CMSG_CAST_SPELL, body }, None)))
        },
        GameplayCommand::UseItemInstance { item: _, item_guid, backpack_slot, spell, target, cast_count } => {
            let mut body = Vec::with_capacity(32);
            body.push(0xff); // INVENTORY_SLOT_BAG_0
            body.push(backpack_slot);
            body.push(cast_count);
            body.extend_from_slice(&spell.to_le_bytes());
            body.extend_from_slice(&item_guid.0.to_le_bytes());
            body.extend_from_slice(&0_u32.to_le_bytes()); // glyph index
            body.push(0); // cast flags
            match target {
                Some(target) => { body.extend_from_slice(&0x0000_0002_u32.to_le_bytes()); push_packed_guid(&mut body, target); }
                None => body.extend_from_slice(&0_u32.to_le_bytes()),
            }
            Ok(Some((ClientFrame { opcode: CMSG_USE_ITEM, body }, None)))
        }
        GameplayCommand::Attack(entity) => Ok(Some((ClientFrame { opcode: CMSG_ATTACKSWING, body: entity.0.to_le_bytes().to_vec() }, None))),
        GameplayCommand::Loot(entity) => Ok(Some((ClientFrame { opcode: CMSG_LOOT, body: entity.0.to_le_bytes().to_vec() }, None))),
        GameplayCommand::AcceptQuest { quest, giver } => {
            let mut body = Vec::with_capacity(16);
            body.extend_from_slice(&giver.0.to_le_bytes()); body.extend_from_slice(&quest.to_le_bytes()); body.extend_from_slice(&0_u32.to_le_bytes());
            Ok(Some((ClientFrame { opcode: CMSG_QUESTGIVER_ACCEPT_QUEST, body }, None)))
        }
        GameplayCommand::TurnInQuest { quest, giver } => {
            let mut body = Vec::with_capacity(12); body.extend_from_slice(&giver.0.to_le_bytes()); body.extend_from_slice(&quest.to_le_bytes());
            Ok(Some((ClientFrame { opcode: CMSG_QUESTGIVER_COMPLETE_QUEST, body }, None)))
        }
        GameplayCommand::RequestQuestReward { quest, giver } => {
            let mut body = Vec::with_capacity(12); body.extend_from_slice(&giver.0.to_le_bytes()); body.extend_from_slice(&quest.to_le_bytes());
            Ok(Some((ClientFrame { opcode: CMSG_QUESTGIVER_REQUEST_REWARD, body }, None)))
        }
        GameplayCommand::ChooseQuestReward { quest, giver, reward } => {
            let mut body = Vec::with_capacity(16); body.extend_from_slice(&giver.0.to_le_bytes()); body.extend_from_slice(&quest.to_le_bytes()); body.extend_from_slice(&reward.to_le_bytes());
            Ok(Some((ClientFrame { opcode: CMSG_QUESTGIVER_CHOOSE_REWARD, body }, None)))
        }
        GameplayCommand::MaintainBuff { spell, target } => {
            const CMSG_CAST_SPELL: u32 = 0x012E;
            let mut body = Vec::with_capacity(24);
            body.push(0);
            body.extend_from_slice(&spell.to_le_bytes());
            body.push(0);
            body.extend_from_slice(&0x0000_0002_u32.to_le_bytes());
            push_packed_guid(&mut body, target);
            Ok(Some((ClientFrame { opcode: CMSG_CAST_SPELL, body }, None)))
        }
        GameplayCommand::Cast { spell, target } | GameplayCommand::VehicleCast { spell, target } => {
            const CMSG_CAST_SPELL: u32 = 0x012E;
            let mut body = Vec::with_capacity(24);
            body.push(0); // cast count
            body.extend_from_slice(&spell.to_le_bytes());
            body.push(0); // cast flags
            match target {
                Some(target) => {
                    body.extend_from_slice(&0x0000_0002_u32.to_le_bytes()); // TARGET_FLAG_UNIT
                    push_packed_guid(&mut body, target);
                }
                None => body.extend_from_slice(&0_u32.to_le_bytes()),
            }
            Ok(Some((ClientFrame { opcode: CMSG_CAST_SPELL, body }, None)))
        }
        GameplayCommand::FaceDirection { orientation } => {
            let guid = player_guid.ok_or_else(|| "facing requested before player GUID is authoritative".to_owned())?;
            let mut position = current.ok_or_else(|| "facing requested before canonical position is known".to_owned())?;
            if !orientation.is_finite() { return Err("facing orientation is invalid".into()); }
            position.orientation = orientation.rem_euclid(std::f32::consts::TAU);
            *movement_time = movement_time.wrapping_add(1).max(1);
            let flags = base_movement_flags & !0x0000_0001_u32;
            let body = encode_simple_movement(guid, flags, *movement_time, position.point, position.orientation);
            Ok(Some((ClientFrame { opcode: MSG_MOVE_SET_FACING, body }, Some((position, false, flags, *movement_time)))))
        }
        GameplayCommand::ReleaseSpirit => Ok(Some((ClientFrame { opcode: 0x015A, body: vec![0] }, None))),
        GameplayCommand::QueryCorpse => Ok(Some((ClientFrame { opcode: 0x0216, body: Vec::new() }, None))),
        GameplayCommand::ReclaimCorpse { player } => Ok(Some((ClientFrame { opcode: 0x01D2, body: player.0.to_le_bytes().to_vec() }, None))),
        GameplayCommand::MoveTo(destination) => {
            let guid = player_guid.ok_or_else(|| "movement requested before player GUID is authoritative".to_owned())?;
            let mut position = current.ok_or_else(|| "movement requested before canonical position is known".to_owned())?;
            if !destination.is_finite() { return Err("movement destination is invalid".into()); }
            let dx = destination.x - position.point.x; let dy = destination.y - position.point.y;
            if dx != 0.0 || dy != 0.0 { position.orientation = dy.atan2(dx); }
            position.point = destination;
            *movement_time = movement_time.wrapping_add(250).max(1);
            // Preserve server-authoritative capabilities such as flying/disable-gravity,
            // while setting forward movement for this step.
            let flags = base_movement_flags | 0x0000_0001_u32;
            let body = encode_simple_movement(guid, flags, *movement_time, position.point, position.orientation);
            Ok(Some((ClientFrame { opcode: MSG_MOVE_HEARTBEAT, body }, Some((position, true, flags, *movement_time)))))
        }
        GameplayCommand::StopMovement => {
            let guid = player_guid.ok_or_else(|| "stop movement requested before player GUID is authoritative".to_owned())?;
            let position = current.ok_or_else(|| "stop movement requested before canonical position is known".to_owned())?;
            *movement_time = movement_time.wrapping_add(1).max(1);
            let flags = base_movement_flags & !0x0000_0001_u32;
            let body = encode_simple_movement(guid, flags, *movement_time, position.point, position.orientation);
            Ok(Some((ClientFrame { opcode: MSG_MOVE_STOP, body }, Some((position, false, flags, *movement_time)))))
        }
        other => Err(format!("{other:?}")),
    }
}

fn push_packed_guid(body: &mut Vec<u8>, guid: EntityId) {
    let bytes = guid.0.to_le_bytes();
    let mut mask = 0_u8;
    let start = body.len();
    body.push(0);
    for (index, byte) in bytes.into_iter().enumerate() {
        if byte != 0 { mask |= 1 << index; body.push(byte); }
    }
    body[start] = mask;
}

fn encode_simple_movement(guid: EntityId, flags: u32, client_time: u32, point: Vec3, orientation: f32) -> Vec<u8> {
    let bytes = guid.0.to_le_bytes();
    let mut mask = 0_u8; let mut body = vec![0_u8];
    for (index, byte) in bytes.into_iter().enumerate() { if byte != 0 { mask |= 1 << index; body.push(byte); } }
    body[0] = mask;
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&0_u16.to_le_bytes());
    body.extend_from_slice(&client_time.to_le_bytes());
    body.extend_from_slice(&point.x.to_le_bytes()); body.extend_from_slice(&point.y.to_le_bytes()); body.extend_from_slice(&point.z.to_le_bytes());
    body.extend_from_slice(&orientation.to_le_bytes()); body.extend_from_slice(&0_u32.to_le_bytes());
    body
}

fn decode_simple_movement(body: &[u8]) -> Option<(EntityId, u32, u32, Vec3, f32)> {
    let mask = *body.first()?; let mut cursor = 1usize; let mut guid = [0_u8;8];
    for (index, slot) in guid.iter_mut().enumerate() { if mask & (1 << index) != 0 { *slot = *body.get(cursor)?; cursor += 1; } }
    let flags = u32::from_le_bytes(body.get(cursor..cursor+4)?.try_into().ok()?); cursor += 4;
    let _extra = u16::from_le_bytes(body.get(cursor..cursor+2)?.try_into().ok()?); cursor += 2;
    let client_time = u32::from_le_bytes(body.get(cursor..cursor+4)?.try_into().ok()?); cursor += 4;
    let x = f32::from_le_bytes(body.get(cursor..cursor+4)?.try_into().ok()?); cursor += 4;
    let y = f32::from_le_bytes(body.get(cursor..cursor+4)?.try_into().ok()?); cursor += 4;
    let z = f32::from_le_bytes(body.get(cursor..cursor+4)?.try_into().ok()?); cursor += 4;
    let o = f32::from_le_bytes(body.get(cursor..cursor+4)?.try_into().ok()?);
    let point = Vec3::new(x,y,z); if !point.is_finite() || !o.is_finite() { return None; }
    Some((EntityId(u64::from_le_bytes(guid)), flags, client_time, point, o))
}

fn maintenance_observations(opcode: u32, body: &[u8]) -> Vec<ProtocolObservation> {
    const SMSG_INITIAL_SPELLS: u32 = 0x012A;
    const SMSG_LEARNED_SPELL: u32 = 0x012B;
    const SMSG_AURA_UPDATE_ALL: u32 = 0x0495;
    const SMSG_AURA_UPDATE: u32 = 0x0496;
    const SMSG_SPELL_COOLDOWN: u32 = 0x0134;
    const MSG_CORPSE_QUERY: u32 = 0x0216;
    const SMSG_CORPSE_RECLAIM_DELAY: u32 = 0x0269;
    match opcode {
        SMSG_INITIAL_SPELLS => parse_initial_spells(body),
        SMSG_LEARNED_SPELL => body.get(0..4).map(|bytes| vec![ProtocolObservation::SpellKnown { spell: u32::from_le_bytes(bytes.try_into().unwrap_or([0;4])) }]).unwrap_or_default(),
        SMSG_AURA_UPDATE_ALL => parse_aura_update_all(body).into_iter().collect(),
        SMSG_AURA_UPDATE => parse_aura_update(body).into_iter().collect(),
        SMSG_SPELL_COOLDOWN => parse_spell_cooldowns(body),
        MSG_CORPSE_QUERY => parse_corpse_query(body).into_iter().collect(),
        SMSG_CORPSE_RECLAIM_DELAY => parse_reclaim_delay(body).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn parse_corpse_query(body:&[u8])->Option<ProtocolObservation>{
    if *body.first()?==0 { return Some(ProtocolObservation::CorpseLocation{position:None}); }
    let map=u32::from_le_bytes(body.get(1..5)?.try_into().ok()?);
    let x=f32::from_le_bytes(body.get(5..9)?.try_into().ok()?);
    let y=f32::from_le_bytes(body.get(9..13)?.try_into().ok()?);
    let z=f32::from_le_bytes(body.get(13..17)?.try_into().ok()?);
    let point=Vec3::new(x,y,z); if !point.is_finite(){return None}
    Some(ProtocolObservation::CorpseLocation{position:Some(WorldPosition{map,point,orientation:0.0})})
}
fn parse_reclaim_delay(body:&[u8])->Option<ProtocolObservation>{
    let delay=u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
    Some(ProtocolObservation::CorpseReclaimDelay{ready_at_ms:wall_clock_ms().saturating_add(u64::from(delay))})
}

fn wall_clock_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn parse_spell_cooldowns(body: &[u8]) -> Vec<ProtocolObservation> {
    if body.len() < 9 { return Vec::new(); }
    let now = wall_clock_ms();
    let mut offset = 9usize; // caster GUID + flags
    let mut out = Vec::new();
    while offset + 8 <= body.len() {
        let spell = u32::from_le_bytes(body[offset..offset+4].try_into().unwrap_or([0;4]));
        let cooldown = u32::from_le_bytes(body[offset+4..offset+8].try_into().unwrap_or([0;4]));
        offset += 8;
        if spell != 0 { out.push(ProtocolObservation::SpellCooldown { spell, ready_at_ms: now.saturating_add(u64::from(cooldown)) }); }
    }
    out
}

fn parse_initial_spells(body: &[u8]) -> Vec<ProtocolObservation> {
    if body.len() < 3 { return Vec::new(); }
    let count = u16::from_le_bytes([body[1], body[2]]) as usize;
    if count > 4096 || body.len() < 3 + count.saturating_mul(6) { return Vec::new(); }
    let mut out = Vec::with_capacity(count);
    let mut offset = 3usize;
    for _ in 0..count {
        let spell = u32::from_le_bytes(body[offset..offset+4].try_into().unwrap_or([0;4]));
        offset += 6;
        if spell != 0 { out.push(ProtocolObservation::SpellKnown { spell }); }
    }
    out
}

fn read_packed_guid(body:&[u8], offset:&mut usize)->Option<EntityId>{
    let mask=*body.get(*offset)?; *offset+=1; let mut bytes=[0_u8;8];
    for (index,slot) in bytes.iter_mut().enumerate(){ if mask&(1<<index)!=0 { *slot=*body.get(*offset)?; *offset+=1; } }
    Some(EntityId(u64::from_le_bytes(bytes)))
}

fn parse_aura_update(body:&[u8])->Option<ProtocolObservation>{
    let mut offset=0usize; let entity=read_packed_guid(body,&mut offset)?; let slot=*body.get(offset)?; offset+=1;
    let spell=u32::from_le_bytes(body.get(offset..offset+4)?.try_into().ok()?);
    let aura=(spell!=0).then_some(wow_state::auras::AuraInstance{slot,spell,positive:None,caster:None,max_duration_ms:None,remaining_ms:None});
    Some(ProtocolObservation::AuraSlot{entity,slot,aura})
}

fn parse_aura_update_all(body:&[u8])->Option<ProtocolObservation>{
    let mut offset=0usize; let entity=read_packed_guid(body,&mut offset)?; let mut auras=Vec::new();
    while offset<body.len(){
        let slot=*body.get(offset)?; offset+=1;
        let spell=u32::from_le_bytes(body.get(offset..offset+4)?.try_into().ok()?); offset+=4;
        let flags=*body.get(offset)?; offset+=1;
        let _level=*body.get(offset)?; offset+=1;
        let caster=if flags&0x08==0 { Some(read_packed_guid(body,&mut offset)?) } else { None };
        let (max_duration_ms,remaining_ms)=if flags&0x20!=0 {
            let max=u32::from_le_bytes(body.get(offset..offset+4)?.try_into().ok()?); offset+=4;
            let remaining=u32::from_le_bytes(body.get(offset..offset+4)?.try_into().ok()?); offset+=4;
            (Some(max),Some(remaining))
        } else {(None,None)};
        if spell!=0 { auras.push(wow_state::auras::AuraInstance{slot,spell,positive:Some(flags&0x10!=0),caster,max_duration_ms,remaining_ms}); }
    }
    Some(ProtocolObservation::AuraSnapshot{entity,auras})
}

fn controlled_abilities_observation(opcode: u32, body: &[u8]) -> Option<ProtocolObservation> {
    const SMSG_PET_SPELLS: u32 = 0x0179;
    if opcode != SMSG_PET_SPELLS || body.len() < 58 { return None; }
    let mover = EntityId(u64::from_le_bytes(body.get(0..8)?.try_into().ok()?));
    if mover.0 == 0 { return None; }
    // AzerothCore VehicleSpellInitialize writes GUID, family, duration, one packed
    // react/command/disable-actions u32, then ten action-bar u32 values.
    let mut offset = 8 + 2 + 4 + 4;
    let mut spells = Vec::new();
    for _ in 0..10 {
        let packed = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?);
        offset += 4;
        let spell = packed & 0x00FF_FFFF;
        if spell != 0 && !spells.contains(&spell) { spells.push(spell); }
    }
    Some(ProtocolObservation::ControlledAbilities { mover, spells })
}

fn quest_observations(opcode: u32, body: &[u8]) -> Vec<ProtocolObservation> {
    const SMSG_QUESTGIVER_STATUS: u32 = 0x0183;
    const SMSG_QUESTGIVER_QUEST_LIST: u32 = 0x0185;
    const SMSG_QUESTGIVER_REQUEST_ITEMS: u32 = 0x018B;
    const SMSG_QUESTGIVER_OFFER_REWARD: u32 = 0x018D;
    const SMSG_QUESTGIVER_STATUS_MULTIPLE: u32 = 0x0418;
    const SMSG_QUEST_QUERY_RESPONSE: u32 = 0x005D;
    match opcode {
        SMSG_QUESTGIVER_STATUS => parse_single_quest_status(body).into_iter().collect(),
        SMSG_QUESTGIVER_STATUS_MULTIPLE => parse_multiple_quest_status(body),
        SMSG_QUESTGIVER_QUEST_LIST => parse_quest_list(body),
        SMSG_QUESTGIVER_REQUEST_ITEMS => parse_quest_request_items(body).into_iter().collect(),
        SMSG_QUESTGIVER_OFFER_REWARD => parse_quest_offer_reward(body).into_iter().collect(),
        SMSG_QUEST_QUERY_RESPONSE => parse_quest_query_response(body).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn parse_single_quest_status(body: &[u8]) -> Option<ProtocolObservation> {
    let giver = EntityId(u64::from_le_bytes(body.get(0..8)?.try_into().ok()?));
    let status = *body.get(8)?;
    Some(ProtocolObservation::QuestGiverStatus { giver, status })
}

fn parse_multiple_quest_status(body: &[u8]) -> Vec<ProtocolObservation> {
    let Some(count_bytes) = body.get(0..4) else { return Vec::new() };
    let count = u32::from_le_bytes(count_bytes.try_into().unwrap_or([0; 4])) as usize;
    if count > 4096 || body.len() < 4 + count.saturating_mul(9) { return Vec::new(); }
    let mut out = Vec::with_capacity(count);
    let mut offset = 4;
    for _ in 0..count {
        let Some(guid_bytes) = body.get(offset..offset + 8) else { break };
        let giver = EntityId(u64::from_le_bytes(guid_bytes.try_into().unwrap_or([0; 8])));
        let Some(status) = body.get(offset + 8).copied() else { break };
        out.push(ProtocolObservation::QuestGiverStatus { giver, status });
        offset += 9;
    }
    out
}

fn parse_quest_list(body: &[u8]) -> Vec<ProtocolObservation> {
    let Some(guid_bytes) = body.get(0..8) else { return Vec::new() };
    let giver = EntityId(u64::from_le_bytes(guid_bytes.try_into().unwrap_or([0; 8])));
    let mut offset = 8;
    if read_cstring(body, &mut offset).is_none() { return Vec::new(); }
    if body.get(offset..offset + 8).is_none() { return Vec::new(); }
    offset += 8; // emote delay + emote type
    let Some(count) = body.get(offset).copied() else { return Vec::new() };
    offset += 1;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let Some(quest_bytes) = body.get(offset..offset + 4) else { break };
        let quest = u32::from_le_bytes(quest_bytes.try_into().unwrap_or([0; 4]));
        let Some(icon_bytes) = body.get(offset + 4..offset + 8) else { break };
        let icon = u32::from_le_bytes(icon_bytes.try_into().unwrap_or([0; 4]));
        if body.get(offset + 8..offset + 17).is_none() { break; }
        offset += 17; // quest, icon, level, flags, repeatable
        if read_cstring(body, &mut offset).is_none() { break; }
        out.push(ProtocolObservation::QuestOffer { giver, quest, icon });
    }
    out
}

fn parse_quest_request_items(body: &[u8]) -> Option<ProtocolObservation> {
    use wow_state::quests::{QuestTurnInDialog, QuestTurnInStage};
    let giver = EntityId(u64::from_le_bytes(body.get(0..8)?.try_into().ok()?));
    let quest = u32::from_le_bytes(body.get(8..12)?.try_into().ok()?);
    if body.len() < 28 { return None; }
    let completion_code = u32::from_le_bytes(body.get(body.len().checked_sub(16)?..body.len().checked_sub(12)?)?.try_into().ok()?);
    Some(ProtocolObservation::QuestTurnInDialog {
        quest,
        dialog: QuestTurnInDialog { giver, stage: QuestTurnInStage::RequestItems { can_complete: completion_code == 3 } },
    })
}

fn parse_quest_offer_reward(body: &[u8]) -> Option<ProtocolObservation> {
    use wow_state::quests::{QuestTurnInDialog, QuestTurnInStage};
    let giver = EntityId(u64::from_le_bytes(body.get(0..8)?.try_into().ok()?));
    let quest = u32::from_le_bytes(body.get(8..12)?.try_into().ok()?);
    let mut offset = 12usize;
    let _title = read_cstring(body, &mut offset)?;
    let _reward_text = read_cstring(body, &mut offset)?;
    offset = offset.checked_add(1 + 4 + 4)?;
    let emote_count = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?) as usize;
    offset = offset.checked_add(4 + emote_count.checked_mul(8)?)?;
    let reward_choices = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?);
    Some(ProtocolObservation::QuestTurnInDialog {
        quest,
        dialog: QuestTurnInDialog { giver, stage: QuestTurnInStage::OfferReward { reward_choices } },
    })
}

fn parse_quest_query_response(body: &[u8]) -> Option<ProtocolObservation> {
    use wow_state::quests::{QuestDefinition, QuestItemObjective, QuestTargetKind, QuestTargetObjective};
    // AzerothCore 3.3.5a writes 65 four-byte fields before the five strings.
    if body.len() < 260 { return None; }
    let read_u32 = |index: usize| -> Option<u32> {
        let offset = index.checked_mul(4)?;
        Some(u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?))
    };
    let quest = read_u32(0)?;
    let poi_map_raw = read_u32(61)?;
    let poi_x = f32::from_bits(read_u32(62)?);
    let poi_y = f32::from_bits(read_u32(63)?);
    let mut offset = 260usize;
    let title = read_cstring(body, &mut offset)?.to_owned();
    let _objectives = read_cstring(body, &mut offset)?;
    let _details = read_cstring(body, &mut offset)?;
    let _area = read_cstring(body, &mut offset)?;
    let _completed = read_cstring(body, &mut offset)?;

    let mut raw_targets = Vec::with_capacity(4);
    for _ in 0..4 {
        let encoded = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?); offset += 4;
        let required = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?); offset += 4;
        let item_drop = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?); offset += 4;
        let _source_count = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?); offset += 4;
        let (kind, entry) = if encoded & 0x8000_0000 != 0 {
            (QuestTargetKind::GameObject, encoded & 0x7FFF_FFFF)
        } else {
            (QuestTargetKind::Creature, encoded)
        };
        raw_targets.push((kind, entry, required, item_drop));
    }
    let mut items = Vec::new();
    for _ in 0..6 {
        let item = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?); offset += 4;
        let required = u32::from_le_bytes(body.get(offset..offset + 4)?.try_into().ok()?); offset += 4;
        if item != 0 && required != 0 { items.push(QuestItemObjective { item, required }); }
    }
    let mut texts = Vec::with_capacity(4);
    for _ in 0..4 { texts.push(read_cstring(body, &mut offset).unwrap_or_default().to_owned()); }
    let targets = raw_targets.into_iter().enumerate().filter_map(|(i, (kind, entry, required, item_drop))| {
        (entry != 0 && required != 0).then(|| QuestTargetObjective {
            slot: i, kind, entry, required, item_drop, text: texts.get(i).cloned().unwrap_or_default(),
        })
    }).collect();
    let poi_valid = poi_map_raw != u32::MAX && poi_x.is_finite() && poi_y.is_finite() && (poi_x != 0.0 || poi_y != 0.0);
    Some(ProtocolObservation::QuestDefinition { definition: QuestDefinition {
        quest,
        title,
        poi_map: poi_valid.then_some(poi_map_raw),
        poi_x: poi_valid.then_some(poi_x),
        poi_y: poi_valid.then_some(poi_y),
        targets,
        items,
    }})
}

fn read_cstring<'a>(body: &'a [u8], offset: &mut usize) -> Option<&'a str> {
    let rest = body.get(*offset..)?;
    let end = rest.iter().position(|b| *b == 0)?;
    let text = std::str::from_utf8(rest.get(..end)?).ok()?;
    *offset += end + 1;
    Some(text)
}

fn movement_matches_bot_visual(visual: WorldPosition, bot_opcode: u32, point: Vec3, orientation: f32, client_opcode: u32) -> bool {
    if !point.is_finite() || !orientation.is_finite() { return false; }
    let delta = (visual.orientation - orientation).rem_euclid(std::f32::consts::TAU);
    let angular = delta.min(std::f32::consts::TAU - delta);
    point.distance(visual.point) <= 0.25
        && angular <= 0.08
        && (client_opcode == bot_opcode || client_opcode == 0x00EE)
}

fn is_player_movement_opcode(opcode: u32) -> bool {
    matches!(opcode,
        0x0B5 | 0x0B6 | 0x0B7 | 0x0B8 | 0x0B9 | 0x0BA | 0x0BB | 0x0BC | 0x0BD | 0x0BE
        | 0x0BF | 0x0C0 | 0x0C1 | 0x0C2 | 0x0C3 | 0x0C5 | 0x0C9 | 0x0CA | 0x0CB
        | 0x0DA | 0x0DB | 0x0EE | 0x359 | 0x35A | 0x3A7)
}

fn is_explicit_player_movement_intent(opcode: u32) -> bool {
    // Start/change opcodes represent direct keyboard/mouse intent. Heartbeat,
    // stop, fall-land, and run/walk-mode packets are state feedback and are
    // not sufficient by themselves to steal locomotion from an active bot.
    matches!(opcode,
        0x0B5 | 0x0B6 | 0x0B8 | 0x0B9 | 0x0BB | 0x0BC | 0x0BD
        | 0x0BF | 0x0C0 | 0x0CA | 0x0DA | 0x0DB | 0x359 | 0x3A7)
}

fn proxy_chat(body: &[u8]) -> Option<(crate::commands::chat::ChatFamily, &str)> {
    use crate::commands::chat::ChatFamily;
    let chat_type = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
    let payload = body.get(8..)?;
    fn cstring(bytes: &[u8]) -> Option<(&str, usize)> {
        let end = bytes.iter().position(|b| *b == 0)?;
        Some((std::str::from_utf8(bytes.get(..end)?).ok()?, end + 1))
    }
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
        1 | 2 | 3 | 4 | 5 | 6 | 10 | 23 | 24 | 39 | 40 | 44 | 45 | 51 => cstring(payload)?.0,
        7 | 17 => { let (_, consumed) = cstring(payload)?; cstring(payload.get(consumed..)?)?.0 }
        _ => return None,
    };
    Some((family, text))
}

async fn handle_log_command(logs: &ActionLogManager, account: &str, command: crate::commands::log::LogCommand) {
    use crate::commands::log::LogCommand;
    match command {
        LogCommand::Start => match logs.start(account).await {
            Ok(path) => tracing::info!(account=%account, path=%path.display(), "action logging started"),
            Err(error) => tracing::error!(account=%account, %error, "failed to start action logging"),
        },
        LogCommand::Stop => match logs.stop(account).await {
            Ok(Some(path)) => tracing::info!(account=%account, path=%path.display(), "action logging stopped"),
            Ok(None) => tracing::info!(account=%account, "action logging is not active"),
            Err(error) => tracing::error!(account=%account, %error, "failed to stop action logging"),
        },
        LogCommand::Status => match logs.status(account).await {
            Some(path) => tracing::info!(account=%account, path=%path.display(), "action logging is active"),
            None => tracing::info!(account=%account, "action logging is inactive"),
        },
        LogCommand::Mark(label) => match logs.mark(account, label.as_deref()).await {
            Ok(true) => tracing::info!(account=%account, ?label, "action log marker written"),
            Ok(false) => tracing::info!(account=%account, "action logging is not active; marker ignored"),
            Err(error) => tracing::error!(account=%account, %error, "failed to write action log marker"),
        },
    }
}

fn port_of(bind: &str) -> Result<u16> { Ok(bind.parse::<SocketAddr>()?.port()) }
fn advertised(host: &str, port: u16) -> String { if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") } }

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
            assert!(matches!(commands::parse_local(extracted, family, true, MissionId(1)).unwrap(), Some(LocalCommand::Log(_))));
        }
    }

    #[test]
    fn matching_bot_facing_echo_does_not_need_human_takeover() {
        let visual = WorldPosition { map: 1, point: Vec3::new(1.0, 2.0, 3.0), orientation: 1.25 };
        assert!(movement_matches_bot_visual(visual, 0x0DA, visual.point, 1.25, 0x0DA));
        assert!(!movement_matches_bot_visual(visual, 0x0DA, Vec3::new(2.0, 2.0, 3.0), 1.25, 0x0DA));
    }

    #[test]
    fn passive_movement_feedback_does_not_count_as_player_intent() {
        for opcode in [0x0B7, 0x0BA, 0x0BE, 0x0C1, 0x0C2, 0x0C3, 0x0C9, 0x0CB, 0x0EE, 0x35A] {
            assert!(is_player_movement_opcode(opcode));
            assert!(!is_explicit_player_movement_intent(opcode), "opcode {opcode:#x} must remain passive");
        }
    }

    #[test]
    fn explicit_start_turn_jump_and_facing_packets_take_player_control() {
        for opcode in [0x0B5, 0x0B6, 0x0B8, 0x0B9, 0x0BB, 0x0BC, 0x0BD, 0x0BF, 0x0C0, 0x0CA, 0x0DA, 0x0DB, 0x359, 0x3A7] {
            assert!(is_player_movement_opcode(opcode));
            assert!(is_explicit_player_movement_intent(opcode), "opcode {opcode:#x} must count as explicit intent");
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
        for _ in 1..10 { body.extend_from_slice(&0_u32.to_le_bytes()); }
        body.push(0); body.push(0);
        let obs = controlled_abilities_observation(0x0179, &body).expect("vehicle spell packet");
        assert!(matches!(obs, ProtocolObservation::ControlledAbilities { mover: EntityId(28511), ref spells } if spells == &vec![51858]));
    }

    #[test]
    fn encodes_gameobject_spell_with_gameobject_target() {
        let mut time = 1;
        let (frame, movement) = encode_gameplay_command(
            GameplayCommand::CastGameObject { spell: 6247, target: EntityId(191609), report_use: true },
            None,
            None,
            0,
            &mut time,
        ).unwrap().unwrap();
        assert_eq!(frame.opcode, 0x012E);
        assert!(movement.is_none());
        assert_eq!(frame.body[0], 0);
        assert_eq!(u32::from_le_bytes(frame.body[1..5].try_into().unwrap()), 6247);
        assert_eq!(u32::from_le_bytes(frame.body[6..10].try_into().unwrap()), 0x0000_0800);
    }

    #[test]
    fn face_direction_uses_wrath_set_facing_movement_opcode() {
        let mut time = 10;
        let current = WorldPosition { map: 1, point: Vec3::new(1.0, 2.0, 3.0), orientation: 0.0 };
        let (frame, movement) = encode_gameplay_command(
            GameplayCommand::FaceDirection { orientation: std::f32::consts::PI },
            Some(EntityId(77)),
            Some(current),
            0,
            &mut time,
        ).unwrap().unwrap();
        assert_eq!(frame.opcode, 0x00DA);
        let (position, moving, _, _) = movement.expect("facing updates canonical movement state");
        assert!(!moving);
        assert!((position.orientation - std::f32::consts::PI).abs() < 1.0e-5);
    }

    #[test]
    fn targeted_spell_correlation_covers_item_and_controlled_casts() {
        assert_eq!(bot_targeted_cast(&GameplayCommand::VehicleCast { spell: 51858, target: Some(EntityId(2)) }), Some((51858, Some(EntityId(2)))));
        assert_eq!(bot_targeted_cast(&GameplayCommand::UseItemInstance { item: 16114, item_guid: EntityId(9), backpack_slot: 3, spell: 19938, target: Some(EntityId(4)), cast_count: 0 }), Some((19938, Some(EntityId(4)))));
        assert_eq!(bot_targeted_cast(&GameplayCommand::CastGameObject { spell: 6247, target: EntityId(5), report_use: true }), Some((6247, Some(EntityId(5)))));
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
        let mut spells=vec![0_u8]; spells.extend_from_slice(&2_u16.to_le_bytes());
        spells.extend_from_slice(&1459_u32.to_le_bytes()); spells.extend_from_slice(&0_u16.to_le_bytes());
        spells.extend_from_slice(&1243_u32.to_le_bytes()); spells.extend_from_slice(&0_u16.to_le_bytes());
        let observed=parse_initial_spells(&spells);
        assert!(observed.iter().any(|o| matches!(o,ProtocolObservation::SpellKnown{spell:1459})));
        assert!(observed.iter().any(|o| matches!(o,ProtocolObservation::SpellKnown{spell:1243})));

        let mut aura=Vec::new(); push_packed_guid(&mut aura,EntityId(7)); aura.push(3); aura.extend_from_slice(&1459_u32.to_le_bytes());
        assert!(matches!(parse_aura_update(&aura),Some(ProtocolObservation::AuraSlot{entity:EntityId(7),slot:3,aura:Some(wow_state::auras::AuraInstance{spell:1459,..})})));
        let mut removed=Vec::new(); push_packed_guid(&mut removed,EntityId(7)); removed.push(3); removed.extend_from_slice(&0_u32.to_le_bytes());
        assert!(matches!(parse_aura_update(&removed),Some(ProtocolObservation::AuraSlot{entity:EntityId(7),slot:3,aura:None})));
    }

    #[test]
    fn encodes_controlled_unit_spell_with_unit_target() {
        let mut time = 1;
        let (frame, movement) = encode_gameplay_command(
            GameplayCommand::VehicleCast { spell: 51858, target: Some(EntityId(28525)) },
            Some(EntityId(28511)),
            None,
            0,
            &mut time,
        ).unwrap().unwrap();
        assert_eq!(frame.opcode, 0x012E);
        assert!(movement.is_none());
        assert_eq!(frame.body[0], 0);
        assert_eq!(u32::from_le_bytes(frame.body[1..5].try_into().unwrap()), 51858);
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
        assert!(matches!(observations.as_slice(), [
            ProtocolObservation::QuestGiverStatus { giver: EntityId(11), status: 8 },
            ProtocolObservation::QuestGiverStatus { giver: EntityId(22), status: 5 },
        ]));
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
        assert!(matches!(observations.as_slice(), [ProtocolObservation::QuestOffer { giver: EntityId(99), quest: 1234, icon: 2 }]));
    }

    #[test]
    fn parses_azerothcore_quest_query_response_objectives() {
        let mut fields = vec![0_u32; 65];
        fields[0] = 1234;
        fields[61] = 0;
        fields[62] = 100.0_f32.to_bits();
        fields[63] = 200.0_f32.to_bits();
        let mut body = Vec::new();
        for value in fields { body.extend_from_slice(&value.to_le_bytes()); }
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
        for _ in 0..5 { body.extend_from_slice(&0_u32.to_le_bytes()); body.extend_from_slice(&0_u32.to_le_bytes()); }
        for text in ["Kill wolves", "Use object", "", ""] { body.extend_from_slice(text.as_bytes()); body.push(0); }

        let observation = parse_quest_query_response(&body).expect("quest definition should parse");
        let ProtocolObservation::QuestDefinition { definition } = observation else { panic!("wrong observation"); };
        assert_eq!(definition.quest, 1234);
        assert_eq!(definition.title, "Quest Title");
        assert_eq!(definition.poi_map, Some(0));
        assert_eq!(definition.targets.len(), 2);
        assert_eq!(definition.targets[0].slot, 0);
        assert_eq!(definition.targets[0].entry, 77);
        assert_eq!(definition.targets[1].slot, 1);
        assert!(matches!(definition.targets[1].kind, wow_state::quests::QuestTargetKind::GameObject));
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
            body.extend_from_slice(&1_u32.to_le_bytes());   // count
            body.extend_from_slice(&200_u32.to_le_bytes()); // display id
            body.extend_from_slice(&0_u32.to_le_bytes());
            body.extend_from_slice(&0_u32.to_le_bytes());
            body.push(0);
        }
        assert_eq!(body.len(), 80);
        let (parsed_guid, gold, slots) = parse_loot_response(&body).expect("loot response should parse");
        assert_eq!(parsed_guid, guid);
        assert_eq!(gold, 7);
        assert_eq!(slots, vec![1, 2, 3]);
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
        assert!(matches!(parsed, ProtocolObservation::QuestTurnInDialog { quest: 42, dialog: wow_state::quests::QuestTurnInDialog { giver: EntityId(77), stage: wow_state::quests::QuestTurnInStage::RequestItems { can_complete: true } } }));

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
        assert!(matches!(parsed, ProtocolObservation::QuestTurnInDialog { quest: 42, dialog: wow_state::quests::QuestTurnInDialog { giver: EntityId(77), stage: wow_state::quests::QuestTurnInStage::OfferReward { reward_choices: 2 } } }));
    }

    #[test]
    fn quest_encoder_matches_azerothcore_handlers() {
        let mut time = 1;
        let (query, _) = encode_gameplay_command(GameplayCommand::QueryQuestGivers, None, None, 0, &mut time).unwrap().unwrap();
        assert_eq!(query.opcode, 0x0417);
        assert!(query.body.is_empty());

        let mut time = 1;
        let (hello, _) = encode_gameplay_command(GameplayCommand::Interact(EntityId(77)), None, None, 0, &mut time).unwrap().unwrap();
        assert_eq!(hello.opcode, 0x0184);
        assert_eq!(hello.body, 77_u64.to_le_bytes());

        let mut time = 1;
        let (accept, _) = encode_gameplay_command(GameplayCommand::AcceptQuest { quest: 42, giver: EntityId(77) }, None, None, 0, &mut time).unwrap().unwrap();
        assert_eq!(accept.opcode, 0x0189);
        assert_eq!(accept.body.len(), 16);
        assert_eq!(&accept.body[0..8], &77_u64.to_le_bytes());
        assert_eq!(&accept.body[8..12], &42_u32.to_le_bytes());
        assert_eq!(&accept.body[12..16], &0_u32.to_le_bytes());

        let mut time = 1;
        let (request_reward, _) = encode_gameplay_command(GameplayCommand::RequestQuestReward { quest: 42, giver: EntityId(77) }, None, None, 0, &mut time).unwrap().unwrap();
        assert_eq!(request_reward.opcode, 0x018C);
        assert_eq!(&request_reward.body[0..8], &77_u64.to_le_bytes());
        assert_eq!(&request_reward.body[8..12], &42_u32.to_le_bytes());

        let mut time = 1;
        let (choose_reward, _) = encode_gameplay_command(GameplayCommand::ChooseQuestReward { quest: 42, giver: EntityId(77), reward: 0 }, None, None, 0, &mut time).unwrap().unwrap();
        assert_eq!(choose_reward.opcode, 0x018E);
        assert_eq!(&choose_reward.body[12..16], &0_u32.to_le_bytes());
    }
}
