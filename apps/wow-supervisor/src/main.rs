use anyhow::{Context, Result, bail};
use local_ip_address::local_ip;
#[cfg(not(target_os = "macos"))]
use rfd::FileDialog;
use std::{
    collections::HashMap,
    env,
    fs::{File, OpenOptions},
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    sync::{Mutex, mpsc},
};
use tracing_subscriber::EnvFilter;
use wow_control_proto::{
    ProxyToWorker, SupervisorCommand, SupervisorWire, WorkerEvent, WorkerToProxy, WorkerWire,
    net::{read_frame, write_frame},
};
use wow_domain::{AccountId, LaneId, WorkerGeneration, time::Millis};
use wow_infra::config::{
    app::{AccountConfig, AppConfig},
    data_dir::AppPaths,
};
use wow_infra::logging::structured::{DiagnosticStream, diagnostic_path};
use wow_proxy::runtime::{ManagedLane, ProxyAccountConfig, ProxyRuntimeConfig};

#[derive(Clone)]
struct TeeMakeWriter {
    file: Arc<std::sync::Mutex<File>>,
}

impl TeeMakeWriter {
    fn new(file: File) -> Self {
        Self {
            file: Arc::new(std::sync::Mutex::new(file)),
        }
    }
}

struct TeeWriter {
    file: Arc<std::sync::Mutex<File>>,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TeeMakeWriter {
    type Writer = TeeWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter {
            file: self.file.clone(),
        }
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut stderr = io::stderr().lock();
        stderr.write_all(buf)?;
        if let Ok(mut file) = self.file.lock() {
            file.write_all(buf)?;
            file.flush()?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()?;
        if let Ok(mut file) = self.file.lock() {
            file.flush()?;
        }
        Ok(())
    }
}

fn init_logging(app_paths: &AppPaths) -> Result<PathBuf> {
    let path = app_paths.logs.join("wow-bot.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open runtime log {}", path.display()))?;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(TeeMakeWriter::new(file))
        .init();
    Ok(path)
}

struct LaneIo {
    worker_to_proxy: mpsc::Sender<WorkerToProxy>,
    proxy_to_worker: Mutex<Option<mpsc::Receiver<ProxyToWorker>>>,
    commands: Mutex<Option<mpsc::Receiver<SupervisorCommand>>>,
    command_tx: mpsc::Sender<SupervisorCommand>,
    generation: WorkerGeneration,
}

#[derive(Clone)]
struct Args {
    config: Option<PathBuf>,
    worker_bin: Option<PathBuf>,
    data_root: Option<PathBuf>,
}
fn args() -> Result<Args> {
    let mut config = None;
    let mut worker_bin = None;
    let mut data_root = None;
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                config = Some(PathBuf::from(
                    it.next().context("--config requires a path")?,
                ))
            }
            "--worker-bin" => {
                worker_bin = Some(PathBuf::from(
                    it.next().context("--worker-bin requires a path")?,
                ))
            }
            "--data-root" => {
                data_root = Some(PathBuf::from(
                    it.next().context("--data-root requires a path")?,
                ))
            }
            "--help" | "-h" => {
                println!(
                    "usage: wow-bot-supervisor [--config path] [--worker-bin path] [--data-root path]"
                );
                println!("default config and writable bot data are stored in <repo>/wow-bot-data");
                println!("set WOW_BOT_HOME to override the repo-local bot-owned data directory");
                std::process::exit(0);
            }
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(Args {
        config,
        worker_bin,
        data_root,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = args()?;
    let app_paths = AppPaths::discover().map_err(anyhow::Error::msg)?;
    app_paths.ensure().map_err(anyhow::Error::msg)?;
    let runtime_log = init_logging(&app_paths)?;
    let run_id = format!("{}-{}", Millis::wall_clock_now().0, std::process::id());
    tracing::info!(runtime_log=%runtime_log.display(), action_logs=%app_paths.logs.display(), "logging initialized");
    let config_path = args.config.clone().unwrap_or_else(|| {
        app_paths
            .root
            .parent()
            .and_then(Path::parent)
            .map(|root| root.join("config.toml"))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| app_paths.config.clone())
    });
    let (mut config, created, persistence_path) = if config_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
    {
        (
            wow_infra::config::source::load_toml(&config_path)?,
            false,
            app_paths.config.clone(),
        )
    } else {
        let (config, created) =
            AppConfig::load_or_create(&config_path).map_err(anyhow::Error::msg)?;
        (config, created, config_path.clone())
    };
    config.runtime.data_dir = app_paths.root.clone();
    if created {
        tracing::info!(config=%config_path.display(), data_root=%app_paths.root.display(), "created first-run configuration in bot-owned storage");
    }
    if persistence_path != config_path {
        tracing::info!(config=%config_path.display(), "loaded user TOML configuration and resolved bot roster");
    }
    ensure_runtime_data(&mut config, &persistence_path, args.data_root.as_deref())?;
    prepare_runtime_asset_manifest(&config, &app_paths)?;
    ensure_interactive_setup(&mut config, &persistence_path, created)?;
    if persistence_path == config_path || created {
        config.save(&persistence_path).map_err(anyhow::Error::msg)?;
    }
    config.validate().map_err(anyhow::Error::msg)?;
    check_upstream_services(&config).await?;
    let worker_bin = resolve_worker_bin(args.worker_bin).await?;

    let listener = TcpListener::bind(&config.runtime.control_bind)
        .await
        .with_context(|| {
            format!(
                "bind worker control listener {}",
                config.runtime.control_bind
            )
        })?;
    tracing::info!(bind=%config.runtime.control_bind, "worker control listener ready");

    let (supervisor_tx, mut supervisor_rx) =
        mpsc::channel::<SupervisorCommand>(config.runtime.worker_queue);
    let mut managed = Vec::new();
    let mut lane_io = HashMap::<LaneId, Arc<LaneIo>>::new();
    for account in config.accounts.iter().filter(|a| a.enabled) {
        let generation = WorkerGeneration(1);
        let (w2p_tx, w2p_rx) = mpsc::channel(config.runtime.worker_queue);
        let (p2w_tx, p2w_rx) = mpsc::channel(config.runtime.worker_queue);
        let (cmd_tx, cmd_rx) = mpsc::channel(config.runtime.worker_queue);
        lane_io.insert(
            account.lane,
            Arc::new(LaneIo {
                worker_to_proxy: w2p_tx,
                proxy_to_worker: Mutex::new(Some(p2w_rx)),
                commands: Mutex::new(Some(cmd_rx)),
                command_tx: cmd_tx,
                generation,
            }),
        );
        managed.push(ManagedLane {
            config: ProxyAccountConfig {
                account: account.account,
                lane: account.lane,
                worker: generation,
                account_name: account.account_name.clone(),
                password: account.password.clone(),
                character: (!account.character.trim().is_empty())
                    .then(|| account.character.clone()),
            },
            worker_rx: w2p_rx,
            worker_tx: p2w_tx,
            supervisor_tx: supervisor_tx.clone(),
        });
    }
    if managed.is_empty() {
        bail!("no enabled accounts are configured");
    }

    let routes = Arc::new(lane_io);
    let accept_routes = routes.clone();
    let (worker_disconnect_tx, mut worker_disconnect_rx) = mpsc::unbounded_channel();
    let mut accept_task =
        tokio::spawn(
            async move { accept_workers(listener, accept_routes, worker_disconnect_tx).await },
        );

    let runtime_paths = config.runtime.runtime_data.resolved();
    let mut children = Vec::new();
    for account in config.accounts.iter().filter(|a| a.enabled) {
        children.push((
            account.lane,
            spawn_worker(
                &worker_bin,
                account.lane,
                WorkerGeneration(1),
                &config.runtime.control_bind,
                &runtime_paths.maps,
                &app_paths.logs,
                &run_id,
            )
            .await?,
        ));
    }

    let dispatch_routes = routes.clone();
    let dispatcher = tokio::spawn(async move {
        while let Some(command) = supervisor_rx.recv().await {
            match &command {
                SupervisorCommand::Shutdown => {
                    for route in dispatch_routes.values() {
                        let _ = route.command_tx.send(command.clone()).await;
                    }
                    break;
                }
                SupervisorCommand::StartLane(lane)
                | SupervisorCommand::StopLane(lane)
                | SupervisorCommand::SetPause { lane, .. }
                | SupervisorCommand::UpdatePause { lane, .. }
                | SupervisorCommand::ReplaceMission { lane, .. } => {
                    if let Some(route) = dispatch_routes.get(lane) {
                        let _ = route.command_tx.send(command).await;
                    }
                }
            }
        }
    });

    let proxy = ProxyRuntimeConfig {
        auth_bind: config.proxy.auth_bind.clone(),
        world_bind: config.proxy.world_bind.clone(),
        transparent_world_bind: config.proxy.transparent_world_bind.clone(),
        advertise_host: config.proxy.advertise_host.clone(),
        upstream_auth_host: config.upstream.auth_host.clone(),
        upstream_auth_port: config.upstream.auth_port,
        upstream_world_host: config.upstream.world_host.clone(),
        upstream_world_port: config.upstream.world_port,
        realm_name: config.upstream.realm_name.clone(),
        handshake_timeout: Duration::from_millis(config.runtime.handshake_timeout_ms),
        max_pre_auth_connections: config.proxy.max_pre_auth_connections,
        max_pre_auth_connections_per_ip: config.proxy.max_pre_auth_connections_per_ip,
        warden_client_image: config.proxy.warden_client_image.clone(),
        log_dir: app_paths.logs.clone(),
        run_id,
    };
    let mut proxy_task = tokio::spawn(wow_proxy::runtime::run(proxy, managed));

    // Workers begin in STARTUP_GATE. Clear it only after listeners and routing exist.
    for lane in routes.keys().copied() {
        supervisor_tx
            .send(SupervisorCommand::StartLane(lane))
            .await
            .ok();
    }
    tracing::info!(accounts = routes.len(), "wow-bot runtime started");

    enum StopReason {
        Proxy(std::result::Result<Result<()>, tokio::task::JoinError>),
        Accept(std::result::Result<Result<()>, tokio::task::JoinError>),
        Signal(&'static str),
        WorkerExited(LaneId, ExitStatus),
        WorkerConnectionEnded(String),
        WorkerMonitorFailed(std::io::Error),
    }

    let mut worker_check = tokio::time::interval(Duration::from_millis(500));
    worker_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let shutdown_signal = shutdown_signal();
    tokio::pin!(shutdown_signal);
    let stop_reason = loop {
        tokio::select! {
            result = &mut proxy_task => break StopReason::Proxy(result),
            result = &mut accept_task => break StopReason::Accept(result),
            signal = &mut shutdown_signal => break StopReason::Signal(signal),
            message = worker_disconnect_rx.recv() => match message {
                Some(message) => break StopReason::WorkerConnectionEnded(message),
                None => break StopReason::WorkerMonitorFailed(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "worker disconnect monitor stopped",
                )),
            },
            _ = worker_check.tick() => match exited_worker(&mut children) {
                Ok(Some((lane, status))) => break StopReason::WorkerExited(lane, status),
                Ok(None) => {},
                Err(error) => break StopReason::WorkerMonitorFailed(error),
            }
        }
    };

    let _ = supervisor_tx.send(SupervisorCommand::Shutdown).await;
    dispatcher.abort();
    proxy_task.abort();
    accept_task.abort();
    for (_, child) in &mut children {
        terminate(child).await;
    }

    match stop_reason {
        StopReason::Proxy(result) => result.context("proxy runtime task failed")??,
        StopReason::Accept(result) => result.context("worker accept task failed")??,
        StopReason::Signal(signal) => tracing::info!(signal, "shutdown requested"),
        StopReason::WorkerExited(lane, status) => {
            bail!("worker for lane {lane} exited unexpectedly with status {status}")
        }
        StopReason::WorkerConnectionEnded(message) => {
            bail!("worker control connection ended unexpectedly: {message}")
        }
        StopReason::WorkerMonitorFailed(error) => {
            return Err(error).context("monitor worker process status");
        }
    }
    Ok(())
}

fn prepare_runtime_asset_manifest(config: &AppConfig, app_paths: &AppPaths) -> Result<()> {
    let resolved = config.runtime.runtime_data.resolved();
    let output = app_paths.generated.join("runtime-assets.json");
    let manifest =
        wow_infra::world_knowledge::generate::generate_runtime_asset_manifest(&resolved, &output)
            .with_context(|| {
            format!(
                "build runtime asset manifest from {}",
                config.runtime.runtime_data.root.display()
            )
        })?;
    tracing::info!(
        path=%output.display(),
        dbc_files=manifest.sources.dbc.files,
        map_files=manifest.sources.maps.files,
        vmap_files=manifest.sources.vmaps.files,
        mmap_files=manifest.sources.mmaps.files,
        map_ids=manifest.map_ids.len(),
        "prepared read-only AzerothCore runtime asset manifest"
    );
    Ok(())
}

fn ensure_runtime_data(
    config: &mut AppConfig,
    config_path: &Path,
    explicit_root: Option<&Path>,
) -> Result<()> {
    if let Some(root) = explicit_root {
        let root = detect_runtime_data_root(root).with_context(|| {
            format!(
                "--data-root does not contain valid AzerothCore runtime data: {}",
                root.display()
            )
        })?;
        persist_runtime_data_root(config, config_path, root)?;
        return Ok(());
    }

    match wow_infra::config::validation::validate_runtime_data(&config.runtime) {
        Ok(()) => return Ok(()),
        Err(error) => tracing::warn!(%error, "AzerothCore runtime data is missing or invalid"),
    }

    loop {
        let start_dir = picker_start_directory(&config.runtime.runtime_data.root);
        let selected = if io::stdin().is_terminal() {
            println!("\nAzerothCore runtime data is not configured for this checkout.");
            println!("Select the folder that contains dbc/, maps/, vmaps/, and mmaps/.");
            println!(
                "You may paste/type the path now, or press Enter to open the native folder browser."
            );
            let typed = prompt_line("AzerothCore data directory [Enter = Browse]: ")?;
            let typed = typed.trim();
            if typed.is_empty() {
                tracing::info!("opening native folder browser for AzerothCore runtime data");
                match pick_runtime_data_folder(start_dir.as_deref()) {
                    Ok(Some(path)) => path,
                    Ok(None) => prompt_runtime_data_path("Folder selection was cancelled.")?,
                    Err(error) => {
                        tracing::warn!(%error, "native folder browser is unavailable");
                        prompt_runtime_data_path("Folder browser is unavailable.")?
                    }
                }
            } else {
                PathBuf::from(typed)
            }
        } else {
            bail!(
                "AzerothCore runtime data is missing or invalid and standard input is not interactive. Pass --data-root <path> to the directory containing dbc/, maps/, vmaps/, and mmaps/"
            );
        };

        match detect_runtime_data_root(&selected) {
            Some(root) => {
                persist_runtime_data_root(config, config_path, root)?;
                return Ok(());
            }
            None => {
                tracing::error!(selected=%selected.display(), "selected folder is not a valid AzerothCore data root; expected non-empty dbc, maps, vmaps, and mmaps directories");
                println!("That folder is not a valid AzerothCore data directory. Try again.");
            }
        }
    }
}

fn prompt_runtime_data_path(reason: &str) -> Result<PathBuf> {
    println!("{reason} Enter the path instead.");
    Ok(PathBuf::from(prompt_required(
        "AzerothCore data directory",
    )?))
}

#[cfg(target_os = "macos")]
fn pick_runtime_data_folder(start_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    use std::process::Command as StdCommand;

    let prompt = "Select AzerothCore data directory (contains dbc, maps, vmaps, and mmaps)";
    let mut choose_folder = format!("set chosenFolder to choose folder with prompt \"{prompt}\"");
    if let Some(dir) = start_dir {
        let escaped = dir
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\"', "\\\"");
        choose_folder.push_str(&format!(" default location POSIX file \"{escaped}\""));
    }
    let script = format!("{choose_folder}\nPOSIX path of chosenFolder");

    let output = StdCommand::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .context("open macOS folder chooser with osascript")?;

    if output.status.success() {
        let value = String::from_utf8(output.stdout)
            .context("macOS folder chooser returned invalid UTF-8")?;
        let value = value.trim();
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(PathBuf::from(value)))
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // AppleScript uses -128 when the user presses Cancel. Treat that as normal cancellation.
        if stderr.contains("-128") || stderr.to_ascii_lowercase().contains("user canceled") {
            Ok(None)
        } else {
            bail!("macOS folder chooser failed: {}", stderr.trim())
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn pick_runtime_data_folder(start_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    let mut dialog = FileDialog::new().set_title("Select AzerothCore data directory");
    if let Some(start_dir) = start_dir {
        dialog = dialog.set_directory(start_dir);
    }
    Ok(dialog.pick_folder())
}

fn detect_runtime_data_root(selected: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::with_capacity(3);
    candidates.push(selected.to_path_buf());

    if selected
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "dbc" | "maps" | "vmaps" | "mmaps"
            )
        })
    {
        if let Some(parent) = selected.parent() {
            candidates.push(parent.to_path_buf());
        }
    }

    candidates.push(selected.join("data"));
    candidates.into_iter().find(|candidate| {
        wow_infra::config::validation::validate_runtime_data_root(candidate).is_ok()
    })
}

fn persist_runtime_data_root(
    config: &mut AppConfig,
    config_path: &Path,
    root: PathBuf,
) -> Result<()> {
    config.runtime.runtime_data.root = root.clone();
    config.runtime.runtime_data.dbc = None;
    config.runtime.runtime_data.maps = None;
    config.runtime.runtime_data.vmaps = None;
    config.runtime.runtime_data.mmaps = None;
    config.save(config_path).map_err(anyhow::Error::msg)?;
    tracing::info!(root=%root.display(), config=%config_path.display(), "saved AzerothCore runtime data path");
    Ok(())
}

fn picker_start_directory(configured: &Path) -> Option<PathBuf> {
    if configured.is_dir() {
        return Some(configured.to_path_buf());
    }
    configured
        .parent()
        .filter(|parent| parent.is_dir())
        .map(Path::to_path_buf)
}

fn ensure_interactive_setup(
    config: &mut AppConfig,
    config_path: &Path,
    created: bool,
) -> Result<()> {
    let needs_account = config.accounts.iter().all(|account| !account.enabled);
    let needs_setup = created || !config.setup_complete || needs_account;
    if !needs_setup {
        return Ok(());
    }

    if !io::stdin().is_terminal() {
        if needs_account {
            bail!(
                "no enabled bot account is configured and standard input is not interactive; run the supervisor in a terminal once to complete setup, or provide a prepared --config file"
            );
        }
        return Ok(());
    }

    println!("\n=== wow-bot first-run setup ===");
    println!(
        "AzerothCore runtime data is treated as read-only. wow-bot writes only to <repo>/wow-bot-data (or WOW_BOT_HOME when explicitly set).\n"
    );

    if created || !config.setup_complete {
        configure_network_topology(config)?;
    }
    if needs_account {
        configure_first_account(config)?;
    }

    config.setup_complete = true;
    config.save(config_path).map_err(anyhow::Error::msg)?;
    println!("\nSetup saved to {}\n", config_path.display());
    Ok(())
}

fn configure_network_topology(config: &mut AppConfig) -> Result<()> {
    let local_server = prompt_yes_no("Is AzerothCore running on this same computer?", true)?;
    if local_server {
        config.upstream.auth_host = "127.0.0.1".into();
        config.upstream.world_host = "127.0.0.1".into();
    } else {
        let host = prompt_required("AzerothCore hostname or IP address")?;
        config.upstream.auth_host = host.clone();
        config.upstream.world_host = host;
    }

    let remote_clients = prompt_yes_no(
        "Will WoW clients on other computers connect to this bot server?",
        false,
    )?;
    if remote_clients {
        let advertised = detect_reachable_local_ip().unwrap_or_else(|| {
            tracing::warn!(
                "could not automatically determine a reachable LAN address for this machine"
            );
            String::new()
        });
        let advertised = if advertised.is_empty() {
            prompt_required("Hostname or LAN IP that those WoW clients can reach")?
        } else {
            println!(
                "Detected this bot server at {advertised}; remote WoW clients will be directed here."
            );
            advertised
        };
        config.proxy.auth_bind = "0.0.0.0:3725".into();
        config.proxy.world_bind = "0.0.0.0:8086".into();
        config.proxy.transparent_world_bind = "0.0.0.0:8088".into();
        config.proxy.advertise_host = advertised;
    } else {
        config.proxy.auth_bind = "127.0.0.1:3725".into();
        config.proxy.world_bind = "127.0.0.1:8086".into();
        config.proxy.transparent_world_bind = "127.0.0.1:8088".into();
        config.proxy.advertise_host = "127.0.0.1".into();
    }
    Ok(())
}

fn detect_reachable_local_ip() -> Option<String> {
    match local_ip().ok()? {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
            Some(ip.to_string())
        }
        _ => None,
    }
}

async fn check_upstream_services(config: &AppConfig) -> Result<()> {
    let timeout = Duration::from_secs(3);
    let auth = format!(
        "{}:{}",
        config.upstream.auth_host, config.upstream.auth_port
    );
    let world = format!(
        "{}:{}",
        config.upstream.world_host, config.upstream.world_port
    );

    let mut failures = Vec::new();
    if let Err(reason) = probe_upstream("AzerothCore auth server", &auth, timeout).await {
        failures.push(reason);
    }
    if let Err(reason) = probe_upstream("AzerothCore world server", &world, timeout).await {
        failures.push(reason);
    }

    if failures.is_empty() {
        tracing::info!(auth=%auth, world=%world, "AzerothCore auth and world services are reachable");
        return Ok(());
    }

    let same_machine =
        config.upstream.auth_host == "127.0.0.1" && config.upstream.world_host == "127.0.0.1";
    let hint = if same_machine {
        "AzerothCore is configured on this same computer. Start both authserver and worldserver, and verify they are listening on the configured ports."
    } else {
        "AzerothCore is configured on another computer. Verify that host is running, the configured ports are correct, and the network/firewall allows this bot server to connect."
    };

    bail!(
        "AzerothCore startup check failed:\n  - {}\n\n{}\nConfigured auth endpoint: {}\nConfigured world endpoint: {}",
        failures.join("\n  - "),
        hint,
        auth,
        world,
    )
}

async fn probe_upstream(
    label: &str,
    endpoint: &str,
    timeout: Duration,
) -> std::result::Result<(), String> {
    match tokio::time::timeout(timeout, TcpStream::connect(endpoint)).await {
        Ok(Ok(stream)) => {
            drop(stream);
            Ok(())
        }
        Ok(Err(error)) => Err(format!("{label} is not reachable at {endpoint}: {error}")),
        Err(_) => Err(format!(
            "{label} did not respond at {endpoint} within {} seconds",
            timeout.as_secs()
        )),
    }
}

fn configure_first_account(config: &mut AppConfig) -> Result<()> {
    println!("\nAdd the first bot account. Internal lane/account IDs are assigned automatically.");
    let account_name = prompt_required("WoW account name")?.to_uppercase();
    let password = loop {
        let value = rpassword::prompt_password("WoW account password: ")?;
        if !value.is_empty() {
            break value;
        }
        println!("Password cannot be empty.");
    };
    let character = prompt_optional("Character name (optional)")?;

    let next_lane = config
        .accounts
        .iter()
        .map(|a| a.lane.get())
        .max()
        .unwrap_or(0)
        + 1;
    let next_account = config
        .accounts
        .iter()
        .map(|a| a.account.get())
        .max()
        .unwrap_or(0)
        + 1;
    config.accounts.push(AccountConfig {
        lane: LaneId::new(next_lane),
        account: AccountId::new(next_account),
        account_name,
        password,
        character,
        enabled: true,
    });
    Ok(())
}

fn prompt_required(label: &str) -> Result<String> {
    loop {
        let value = prompt_line(&format!("{label}: "))?;
        let value = value.trim();
        if !value.is_empty() {
            return Ok(value.to_owned());
        }
        println!("A value is required.");
    }
}

fn prompt_optional(label: &str) -> Result<String> {
    Ok(prompt_line(&format!("{label}: "))?.trim().to_owned())
}

fn prompt_yes_no(label: &str, default: bool) -> Result<bool> {
    let suffix = if default { " [Y/n]: " } else { " [y/N]: " };
    loop {
        let value = prompt_line(&format!("{label}{suffix}"))?;
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() {
            return Ok(default);
        }
        match value.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Please answer yes or no."),
        }
    }
}

fn prompt_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value)
}

async fn accept_workers(
    listener: TcpListener,
    routes: Arc<HashMap<LaneId, Arc<LaneIo>>>,
    disconnect_tx: mpsc::UnboundedSender<String>,
) -> Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let routes = routes.clone();
        let disconnect_tx = disconnect_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = attach_worker(stream, routes, disconnect_tx).await {
                tracing::warn!(%peer, error=%format_args!("{e:#}"), "worker control connection ended");
            }
        });
    }
}

async fn attach_worker(
    stream: TcpStream,
    routes: Arc<HashMap<LaneId, Arc<LaneIo>>>,
    disconnect_tx: mpsc::UnboundedSender<String>,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();
    let hello: WorkerWire = read_frame(&mut reader).await?;
    let (lane, generation) = match hello {
        WorkerWire::Hello { lane, generation } => (lane, generation),
        _ => bail!("first worker frame was not Hello"),
    };
    let route = routes
        .get(&lane)
        .with_context(|| format!("worker connected for unknown lane {lane}"))?
        .clone();
    if route.generation != generation {
        bail!(
            "stale worker generation {} for lane {}",
            generation.get(),
            lane
        );
    }
    let mut p2w = route
        .proxy_to_worker
        .lock()
        .await
        .take()
        .context("lane already has a worker connection")?;
    let mut commands = route
        .commands
        .lock()
        .await
        .take()
        .context("lane command receiver already attached")?;
    let w2p = route.worker_to_proxy.clone();
    tracing::info!(%lane, generation=generation.get(), "worker attached");

    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                value = p2w.recv() => match value { Some(v)=>write_frame(&mut writer, &SupervisorWire::Proxy(v)).await?, None=>break },
                value = commands.recv() => match value { Some(v)=>write_frame(&mut writer, &SupervisorWire::Command(v)).await?, None=>break },
            }
        }
        Result::<()>::Ok(())
    });

    loop {
        match read_frame::<_, WorkerWire>(&mut reader).await {
            Ok(WorkerWire::Proxy(message)) => {
                if w2p.send(message).await.is_err() {
                    writer_task.abort();
                    let message = worker_disconnect_message(lane, generation, "proxy route closed");
                    let _ = disconnect_tx.send(message.clone());
                    bail!(message);
                }
            }
            Ok(WorkerWire::Event(event)) => log_worker_event(event),
            Ok(WorkerWire::Hello { .. }) => bail!("worker sent duplicate Hello"),
            Err(e) => {
                writer_task.abort();
                let message = worker_disconnect_message(lane, generation, &e.to_string());
                let _ = disconnect_tx.send(message.clone());
                return Err(e).context(message);
            }
        }
    }
}

fn log_worker_event(event: WorkerEvent) {
    match event {
        WorkerEvent::Ready { lane, generation } => {
            tracing::info!(%lane, generation=generation.get(), "worker ready")
        }
        WorkerEvent::Heartbeat { lane, generation } => {
            tracing::trace!(%lane, generation=generation.get(), "worker heartbeat")
        }
        WorkerEvent::Failed {
            lane,
            generation,
            reason,
        } => tracing::error!(%lane, generation=generation.get(), %reason, "worker failed"),
        WorkerEvent::Stopped { lane, generation } => {
            tracing::info!(%lane, generation=generation.get(), "worker stopped")
        }
    }
}

async fn spawn_worker(
    path: &Path,
    lane: LaneId,
    generation: WorkerGeneration,
    control: &str,
    maps_dir: &Path,
    log_dir: &Path,
    run_id: &str,
) -> Result<Child> {
    tracing::info!(%lane, worker=%path.display(), "starting worker process");
    let mut child = Command::new(path)
        .kill_on_drop(true)
        .arg("--lane")
        .arg(lane.to_string())
        .arg("--generation")
        .arg(generation.get().to_string())
        .arg("--control")
        .arg(control)
        .arg("--maps-dir")
        .arg(maps_dir)
        .env("WOW_BOT_RUN_ID", run_id)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(DiagnosticStream::WORKER.into_iter().filter_map(|stream| {
            Some((
                stream.worker_env_key()?,
                diagnostic_path(log_dir, stream, lane),
            ))
        }))
        .spawn()
        .with_context(|| format!("start worker {}", path.display()))?;

    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(%lane, worker_stream="stdout", message=%line, "worker log");
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(%lane, worker_stream="stderr", message=%line, "worker log");
            }
        });
    }
    Ok(child)
}

fn exited_worker(
    children: &mut [(LaneId, Child)],
) -> std::io::Result<Option<(LaneId, ExitStatus)>> {
    for (lane, child) in children {
        if let Some(status) = child.try_wait()? {
            return Ok(Some((*lane, status)));
        }
    }
    Ok(None)
}

fn worker_disconnect_message(lane: LaneId, generation: WorkerGeneration, reason: &str) -> String {
    format!("lane {lane} generation {}: {reason}", generation.get())
}

async fn resolve_worker_bin(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path);
        }
        bail!("worker binary does not exist: {}", path.display());
    }

    let worker_name = if cfg!(windows) {
        "wow-bot-worker.exe"
    } else {
        "wow-bot-worker"
    };
    let current_exe = env::current_exe()?;
    let profile = current_exe
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| *name == "debug" || *name == "release")
        .unwrap_or(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        });

    // Prefer the checkout the user is currently running from. This avoids a
    // stale build-time or previously moved repository path winning discovery.
    let workspace_root = env::current_dir()
        .ok()
        .and_then(|cwd| wow_infra::config::data_dir::find_workspace_root(&cwd));
    let mut candidates = Vec::new();

    if let Some(root) = workspace_root.as_ref() {
        if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR") {
            candidates.push(PathBuf::from(target_dir).join(profile).join(worker_name));
        }
        candidates.push(root.join("target").join(profile).join(worker_name));
    }

    if let Some(parent) = current_exe.parent() {
        candidates.push(parent.join(worker_name));
    }

    for candidate in &candidates {
        if candidate.is_file() {
            tracing::info!(worker=%candidate.display(), "resolved worker binary");
            return Ok(candidate.clone());
        }
    }

    // In a source checkout, `cargo run -p wow-bot-supervisor` should be enough.
    // Build only the missing worker package and then continue startup.
    if let Some(root) = workspace_root.as_ref() {
        tracing::warn!(workspace=%root.display(), "worker binary is missing; building wow-bot-worker automatically");
        let mut command = Command::new("cargo");
        command
            .current_dir(root)
            .arg("build")
            .arg("-p")
            .arg("wow-bot-worker");
        if profile == "release" {
            command.arg("--release");
        }
        let status = command
            .status()
            .await
            .context("run cargo to build wow-bot-worker")?;
        if !status.success() {
            bail!(
                "automatic worker build failed with status {status}; run `cargo build -p wow-bot-worker{}` in {}",
                if profile == "release" {
                    " --release"
                } else {
                    ""
                },
                root.display()
            );
        }
        let candidate = root.join("target").join(profile).join(worker_name);
        if candidate.is_file() {
            tracing::info!(worker=%candidate.display(), "built and resolved worker binary");
            return Ok(candidate);
        }
    }

    let searched = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "wow-bot-worker was not found. Searched: {searched}. Run `cargo build -p wow-bot-worker`, install the worker beside the supervisor, or pass --worker-bin <path>"
    )
}

async fn terminate(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(unix)]
async fn shutdown_signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate()).expect("register SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => "SIGINT",
        _ = terminate.recv() => "SIGTERM",
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> &'static str {
    let _ = tokio::signal::ctrl_c().await;
    "CTRL-C"
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worker_monitor_reports_the_lane_for_an_exited_child() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 7")
            .spawn()
            .expect("start test worker");
        child.wait().await.expect("wait for test worker");
        let mut children = vec![(LaneId(4), child)];

        let exited = exited_worker(&mut children).expect("check worker status");

        assert_eq!(
            exited.map(|(lane, status)| (lane, status.code())),
            Some((LaneId(4), Some(7)))
        );
    }

    #[test]
    fn worker_disconnect_message_identifies_lane_and_generation() {
        assert_eq!(
            worker_disconnect_message(LaneId(4), WorkerGeneration(2), "unexpected end of file"),
            "lane 4 generation 2: unexpected end of file"
        );
    }
}
