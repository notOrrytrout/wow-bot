use super::{
    app::{
        AccountConfig, AppConfig, DEFAULT_LOOPBACK_HOST, DEFAULT_PROXY_AUTH_PORT,
        DEFAULT_PROXY_BIND_HOST, DEFAULT_PROXY_WORLD_PORT, DEFAULT_TRANSPARENT_WORLD_PORT,
        DEFAULT_UPSTREAM_AUTH_PORT, DEFAULT_UPSTREAM_WORLD_PORT, ProxyConfig, UpstreamConfig,
    },
    runtime_data::RuntimeTuning,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};
use wow_domain::{AccountId, LaneId};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BotRoster {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    accounts: Vec<RosterAccount>,
    #[serde(default)]
    bots: Vec<RosterBot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterAccount {
    id: String,
    username: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    password_env: Option<String>,
    #[serde(default)]
    use_proxy_credentials: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterBot {
    id: String,
    account: String,
    character: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    creation: Option<toml::Value>,
    #[serde(default)]
    realm_name: Option<String>,
}

fn default_enabled() -> bool {
    true
}

fn table<'a>(root: &'a toml::Value, path: &[&str]) -> Option<&'a toml::Value> {
    path.iter().try_fold(root, |value, key| value.get(*key))
}

fn string(root: &toml::Value, path: &[&str]) -> Option<String> {
    table(root, path)?.as_str().map(str::to_owned)
}

fn number(root: &toml::Value, path: &[&str]) -> Option<u64> {
    table(root, path)?
        .as_integer()
        .and_then(|n| u64::try_from(n).ok())
}

fn endpoint(
    root: &toml::Value,
    bind_path: &[&str],
    port_path: &[&str],
    default_port: u64,
) -> String {
    if let Some(bind) = string(root, bind_path).filter(|value| !value.trim().is_empty()) {
        return bind;
    }
    let port = number(root, port_path).unwrap_or(default_port);
    format!("{DEFAULT_PROXY_BIND_HOST}:{port}")
}

fn resolve_from(parent: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        parent.join(path)
    }
}

fn secure_input_file(path: &Path) -> Result<()> {
    crate::security::paths::reject_symlink_path(path).with_context(|| {
        format!(
            "credential file must not use a symbolic link: {}",
            path.display()
        )
    })?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("inspect credential file {}", path.display()))?;
    if !metadata.is_file() {
        bail!("credential path is not a regular file: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
                format!(
                    "restrict credential file {} to owner access",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Load the user-edited TOML config and roster, then build the supervisor's runtime config.
/// The JSON `AppConfig` remains a private runtime representation only.
pub fn load_toml(path: impl AsRef<Path>) -> Result<AppConfig> {
    let path = path.as_ref();
    secure_input_file(path).with_context(|| format!("secure TOML config {}", path.display()))?;
    let source =
        fs::read_to_string(path).with_context(|| format!("read TOML config {}", path.display()))?;
    let values: toml::Value =
        toml::from_str(&source).with_context(|| format!("parse TOML config {}", path.display()))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    let roster_path = string(&values, &["bots", "roster_file"])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("bots.toml"));
    let roster_path = resolve_from(parent, roster_path);
    secure_input_file(&roster_path)
        .with_context(|| format!("secure bot roster {}", roster_path.display()))?;
    let roster_text = fs::read_to_string(&roster_path)
        .with_context(|| format!("read bot roster {}", roster_path.display()))?;
    let roster: BotRoster = toml::from_str(&roster_text)
        .with_context(|| format!("parse bot roster {}", roster_path.display()))?;
    if roster.version.is_some_and(|version| version != 1) {
        bail!(
            "unsupported bot roster version in {}",
            roster_path.display()
        );
    }

    let proxy_client_host =
        string(&values, &["proxy_client_host"]).unwrap_or_else(|| DEFAULT_LOOPBACK_HOST.to_owned());
    let wow_server_host =
        string(&values, &["wow_server_host"]).unwrap_or_else(|| DEFAULT_LOOPBACK_HOST.to_owned());
    let auth_host = string(&values, &["proxy", "upstream", "auth_host"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| wow_server_host.clone());
    let world_host = string(&values, &["proxy", "upstream", "world_host"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| wow_server_host.clone());
    let realm_name = string(&values, &["proxy", "upstream", "realm_name"])
        .or_else(|| string(&values, &["wow", "realm_name"]))
        .unwrap_or_else(|| "AzerothCore".to_owned());
    let data_dir = string(&values, &["data_dir"])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"));
    let data_dir = resolve_from(parent, data_dir);

    let mut config = AppConfig::default();
    config.setup_complete = true;
    if let Some(database_url_env) = string(&values, &["memory", "database_url_env"]) {
        config.memory.database_url_env = database_url_env;
    }
    config.memory.required = values
        .get("memory")
        .and_then(|section| section.get("required"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(config.memory.required);
    config.debug.enabled = values
        .get("debug")
        .and_then(|section| section.get("enabled"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(config.debug.enabled);
    config.runtime.control_bind =
        string(&values, &["server", "bind"]).unwrap_or_else(|| config.runtime.control_bind.clone());
    config.runtime.worker_queue = number(&values, &["proxy", "limits", "worker_command_capacity"])
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(config.runtime.worker_queue);
    config.runtime.handshake_timeout_ms =
        number(&values, &["proxy", "limits", "handshake_timeout_ms"])
            .unwrap_or(config.runtime.handshake_timeout_ms);
    if let Some(tuning) = values.get("runtime_tuning") {
        config.runtime.runtime_tuning = tuning
            .clone()
            .try_into::<RuntimeTuning>()
            .context("parse runtime_tuning settings")?;
    }
    config.runtime.runtime_data.root = data_dir;
    config.runtime.runtime_data.dbc = string(&values, &["wow", "dbc_dir"])
        .map(PathBuf::from)
        .map(|path| resolve_from(parent, path));
    config.runtime.runtime_data.maps = string(&values, &["wow", "maps_dir"])
        .map(PathBuf::from)
        .map(|path| resolve_from(parent, path));
    config.runtime.runtime_data.vmaps = string(&values, &["wow", "vmaps_dir"])
        .map(PathBuf::from)
        .map(|path| resolve_from(parent, path));
    config.runtime.runtime_data.mmaps = string(&values, &["wow", "mmaps_dir"])
        .map(PathBuf::from)
        .map(|path| resolve_from(parent, path));

    config.upstream = UpstreamConfig {
        auth_host,
        auth_port: number(&values, &["proxy", "upstream", "auth_port"])
            .and_then(|n| u16::try_from(n).ok())
            .unwrap_or(DEFAULT_UPSTREAM_AUTH_PORT),
        world_host,
        world_port: number(&values, &["proxy", "upstream", "world_port"])
            .and_then(|n| u16::try_from(n).ok())
            .unwrap_or(DEFAULT_UPSTREAM_WORLD_PORT),
        realm_name,
    };
    let player_auth_port = number(&values, &["proxy", "player", "auth_port"])
        .unwrap_or(u64::from(DEFAULT_PROXY_AUTH_PORT));
    let player_world_port = number(&values, &["proxy", "player", "world_port"])
        .unwrap_or(u64::from(DEFAULT_PROXY_WORLD_PORT));
    let passthrough_port = number(&values, &["proxy", "account", "passthrough_world_port"])
        .unwrap_or(u64::from(DEFAULT_TRANSPARENT_WORLD_PORT));
    config.proxy = ProxyConfig {
        auth_bind: endpoint(
            &values,
            &["proxy", "player", "auth_bind"],
            &["proxy", "player", "auth_port"],
            player_auth_port,
        ),
        world_bind: endpoint(
            &values,
            &["proxy", "player", "world_bind"],
            &["proxy", "player", "world_port"],
            player_world_port,
        ),
        transparent_world_bind: format!("{DEFAULT_PROXY_BIND_HOST}:{passthrough_port}"),
        advertise_host: string(&values, &["proxy", "player", "advertise_host"])
            .filter(|value| !value.is_empty())
            .unwrap_or(proxy_client_host),
        max_pre_auth_connections: number(&values, &["proxy", "limits", "max_pre_auth_connections"])
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(config.proxy.max_pre_auth_connections),
        max_pre_auth_connections_per_ip: number(
            &values,
            &["proxy", "limits", "max_pre_auth_connections_per_ip"],
        )
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(config.proxy.max_pre_auth_connections_per_ip),
        warden_client_image: string(&values, &["proxy", "warden_client_image"]).map(PathBuf::from),
    };

    let proxy_password = string(&values, &["proxy", "account", "password"]);
    let proxy_password_env = string(&values, &["proxy", "account", "password_env"]);
    let mut active_accounts = HashSet::new();
    let mut names = HashSet::new();
    let mut configured_accounts = Vec::<(&RosterAccount, Option<&RosterBot>)>::new();
    for bot in roster.bots.iter().filter(|bot| bot.enabled) {
        let _ = (&bot.creation, &bot.realm_name);
        if !active_accounts.insert(bot.account.clone()) {
            bail!(
                "enabled bots use account {:?} more than once; this supervisor starts one session per account",
                bot.account
            );
        }
    }
    for bot in &roster.bots {
        if configured_accounts
            .iter()
            .any(|(account, _)| account.id == bot.account)
        {
            continue;
        }
        let account = roster
            .accounts
            .iter()
            .find(|account| account.id == bot.account)
            .with_context(|| {
                format!(
                    "bot {:?} references missing account {:?}",
                    bot.id, bot.account
                )
            })?;
        let representative = roster
            .bots
            .iter()
            .find(|candidate| candidate.account == bot.account && candidate.enabled)
            .unwrap_or(bot);
        configured_accounts.push((account, Some(representative)));
    }
    if configured_accounts.is_empty() {
        bail!(
            "bot roster {} has no [[bots]] entries",
            roster_path.display()
        );
    }
    for (account, bot) in configured_accounts {
        let password = account
            .password
            .clone()
            .or_else(|| {
                account
                    .password_env
                    .as_deref()
                    .and_then(|name| env::var(name).ok())
            })
            .or_else(|| {
                account
                    .use_proxy_credentials
                    .then(|| proxy_password.clone())
                    .flatten()
            })
            .or_else(|| {
                account
                    .use_proxy_credentials
                    .then(|| {
                        proxy_password_env
                            .as_deref()
                            .and_then(|name| env::var(name).ok())
                    })
                    .flatten()
            })
            .with_context(|| {
                format!(
                    "account {:?} needs password or password_env in bots.toml",
                    account.id
                )
            })?;
        if !names.insert(account.username.to_ascii_uppercase()) {
            bail!(
                "bot roster contains duplicate account username {:?}",
                account.username
            );
        }
        let index = u64::try_from(config.accounts.len() + 1).context("too many routed accounts")?;
        config.accounts.push(AccountConfig {
            lane: LaneId::new(index),
            account: AccountId::new(index),
            account_name: account.username.clone(),
            bot_id: bot
                .map(|entry| entry.id.clone())
                .unwrap_or_else(|| account.username.clone()),
            password,
            character: bot.map(|entry| entry.character.clone()).unwrap_or_default(),
            enabled: active_accounts.contains(&account.id),
        });
    }
    config.validate().map_err(anyhow::Error::msg)?;
    tracing::info!(config=%path.display(), roster=%roster_path.display(), accounts=config.accounts.len(), "loaded user TOML configuration and bot roster");
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn legacy_toml_uses_shared_ports_and_runtime_tuning_defaults() {
        let unique = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let directory = Path::new("/private/tmp").join(format!(
            "wow-infra-runtime-tuning-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let roster_path = directory.join("bots.toml");
        fs::write(
            &config_path,
            r#"
[bots]
roster_file = "bots.toml"

[memory]
database_url_env = "TEST_BOT_DB_URL"
required = false

[debug]
enabled = true

[runtime_tuning.movement]
travel_speed_form_min_yards = 30
path_straightness = 0.75

[runtime_tuning.maintenance]
auto_mount_enabled = true
mount_min_travel_yards = 80

[runtime_tuning.group]
threat_delay_ms = 4200
"#,
        )
        .unwrap();
        fs::write(
            &roster_path,
            r#"
[[accounts]]
id = "test-account"
username = "test-user"
password = "test-password"

[[bots]]
id = "test-bot"
account = "test-account"
character = "Test Character"
"#,
        )
        .unwrap();

        let config = load_toml(&config_path).unwrap();
        let defaults = AppConfig::default();
        assert_eq!(config.memory.database_url_env, "TEST_BOT_DB_URL");
        assert!(!config.memory.required);
        assert!(config.debug.enabled);
        assert_eq!(config.accounts[0].bot_id, "test-bot");
        assert_eq!(config.upstream.auth_port, defaults.upstream.auth_port);
        assert_eq!(config.upstream.world_port, defaults.upstream.world_port);
        assert_eq!(config.proxy.auth_bind, defaults.proxy.auth_bind);
        assert_eq!(config.proxy.world_bind, defaults.proxy.world_bind);
        assert_eq!(
            config.proxy.transparent_world_bind,
            defaults.proxy.transparent_world_bind
        );
        assert_eq!(
            config
                .runtime
                .runtime_tuning
                .movement
                .travel_speed_form_min_yards,
            30
        );
        assert_eq!(
            config.runtime.runtime_tuning.movement.path_straightness,
            0.75
        );
        assert!(config.runtime.runtime_tuning.maintenance.auto_mount_enabled);
        assert_eq!(config.runtime.runtime_tuning.group.threat_delay_ms(), 4_200);
        assert_eq!(
            config
                .runtime
                .runtime_tuning
                .maintenance
                .mount_min_travel_yards,
            80
        );

        fs::remove_dir_all(directory).unwrap();
    }
}
