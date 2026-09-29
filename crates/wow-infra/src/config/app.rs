use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use wow_domain::{AccountId, LaneId};

pub const DEFAULT_LOOPBACK_HOST: &str = "127.0.0.1";
pub const DEFAULT_PROXY_BIND_HOST: &str = "0.0.0.0";
pub const DEFAULT_UPSTREAM_AUTH_PORT: u16 = 3724;
pub const DEFAULT_UPSTREAM_WORLD_PORT: u16 = 8085;
pub const DEFAULT_PROXY_AUTH_PORT: u16 = 3725;
pub const DEFAULT_PROXY_WORLD_PORT: u16 = 8086;
pub const DEFAULT_TRANSPARENT_WORLD_PORT: u16 = 8088;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub lane: LaneId,
    pub account: AccountId,
    pub account_name: String,
    #[serde(default)]
    pub bot_id: String,
    pub password: String,
    pub character: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl std::fmt::Debug for AccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountConfig")
            .field("lane", &self.lane)
            .field("account", &self.account)
            .field("account_name", &self.account_name)
            .field("bot_id", &self.bot_id)
            .field("password", &"[redacted]")
            .field("character", &self.character)
            .field("enabled", &self.enabled)
            .finish()
    }
}

const fn default_true() -> bool {
    true
}
const fn default_auth_port() -> u16 {
    DEFAULT_UPSTREAM_AUTH_PORT
}
const fn default_world_port() -> u16 {
    DEFAULT_UPSTREAM_WORLD_PORT
}
fn default_proxy_auth_bind() -> String {
    listener_bind(DEFAULT_PROXY_BIND_HOST, DEFAULT_PROXY_AUTH_PORT)
}
fn default_proxy_world_bind() -> String {
    listener_bind(DEFAULT_PROXY_BIND_HOST, DEFAULT_PROXY_WORLD_PORT)
}
fn default_transparent_world_bind() -> String {
    listener_bind(DEFAULT_PROXY_BIND_HOST, DEFAULT_TRANSPARENT_WORLD_PORT)
}
fn default_advertise_host() -> String {
    DEFAULT_LOOPBACK_HOST.to_owned()
}
fn listener_bind(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}
const fn default_max_total() -> usize {
    128
}
const fn default_max_per_ip() -> usize {
    8
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamConfig {
    pub auth_host: String,
    #[serde(default = "default_auth_port")]
    pub auth_port: u16,
    pub world_host: String,
    #[serde(default = "default_world_port")]
    pub world_port: u16,
    pub realm_name: String,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            auth_host: DEFAULT_LOOPBACK_HOST.into(),
            auth_port: default_auth_port(),
            world_host: DEFAULT_LOOPBACK_HOST.into(),
            world_port: default_world_port(),
            realm_name: "AzerothCore".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    #[serde(default = "default_proxy_auth_bind")]
    pub auth_bind: String,
    #[serde(default = "default_proxy_world_bind")]
    pub world_bind: String,
    #[serde(default = "default_transparent_world_bind")]
    pub transparent_world_bind: String,
    #[serde(default = "default_advertise_host")]
    pub advertise_host: String,
    #[serde(default = "default_max_total")]
    pub max_pre_auth_connections: usize,
    #[serde(default = "default_max_per_ip")]
    pub max_pre_auth_connections_per_ip: usize,
    #[serde(default)]
    pub warden_client_image: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default)]
    pub setup_complete: bool,
    #[serde(default)]
    pub runtime: super::runtime_data::RuntimeConfig,
    pub upstream: UpstreamConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub debug: DebugConfig,
}

fn default_memory_database_url_env() -> String {
    "TENTACLI_BOT_DATABASE_URL".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_database_url_env")]
    pub database_url_env: String,
    #[serde(default)]
    pub required: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            database_url_env: default_memory_database_url_env(),
            required: false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugConfig {
    #[serde(default)]
    pub enabled: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            auth_bind: default_proxy_auth_bind(),
            world_bind: default_proxy_world_bind(),
            transparent_world_bind: default_transparent_world_bind(),
            advertise_host: default_advertise_host(),
            max_pre_auth_connections: default_max_total(),
            max_pre_auth_connections_per_ip: default_max_per_ip(),
            warden_client_image: None,
        }
    }
}

impl ProxyConfig {
    pub fn set_listener_bind_host(&mut self, host: &str) {
        self.auth_bind = listener_bind(host, DEFAULT_PROXY_AUTH_PORT);
        self.world_bind = listener_bind(host, DEFAULT_PROXY_WORLD_PORT);
        self.transparent_world_bind = listener_bind(host, DEFAULT_TRANSPARENT_WORLD_PORT);
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            setup_complete: false,
            runtime: super::runtime_data::RuntimeConfig::default(),
            upstream: UpstreamConfig::default(),
            proxy: ProxyConfig::default(),
            accounts: Vec::new(),
            memory: MemoryConfig::default(),
            debug: DebugConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_matches_serialized_defaults() {
        let expected = serde_json::to_string_pretty(&AppConfig::default())
            .expect("default config should serialize")
            + "\n";

        assert_eq!(include_str!("../../../../config.example.json"), expected);
    }

    #[test]
    fn proxy_listener_bind_host_keeps_shared_ports() {
        let mut proxy = ProxyConfig::default();

        proxy.set_listener_bind_host(DEFAULT_LOOPBACK_HOST);

        assert_eq!(
            proxy.auth_bind,
            format!("{DEFAULT_LOOPBACK_HOST}:{DEFAULT_PROXY_AUTH_PORT}")
        );
        assert_eq!(
            proxy.world_bind,
            format!("{DEFAULT_LOOPBACK_HOST}:{DEFAULT_PROXY_WORLD_PORT}")
        );
        assert_eq!(
            proxy.transparent_world_bind,
            format!("{DEFAULT_LOOPBACK_HOST}:{DEFAULT_TRANSPARENT_WORLD_PORT}")
        );

        proxy.set_listener_bind_host(DEFAULT_PROXY_BIND_HOST);
        assert_eq!(proxy.auth_bind, default_proxy_auth_bind());
        assert_eq!(proxy.world_bind, default_proxy_world_bind());
        assert_eq!(
            proxy.transparent_world_bind,
            default_transparent_world_bind()
        );
    }
}

impl AppConfig {
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to create config directory {}: {e}",
                    parent.display()
                )
            })?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize config: {e}"))?;
        #[cfg(unix)]
        crate::security::paths::secure_private_file(path)
            .map_err(|e| format!("failed to secure private config {}: {e}", path.display()))?;
        fs::write(path, format!("{body}\n"))
            .map_err(|e| format!("failed to write {}: {e}", path.display()))
    }

    pub fn load_or_create(path: impl AsRef<Path>) -> Result<(Self, bool), String> {
        let path = path.as_ref();
        if path.exists() {
            return Self::load(path).map(|config| (config, false));
        }
        let config = Self::default();
        config.save(path)?;
        Ok((config, true))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let body = fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let config: Self = serde_json::from_str(&body)
            .map_err(|e| format!("failed to parse {} as JSON: {e}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        super::validation::validate(&self.runtime)?;
        if self.memory.database_url_env.trim().is_empty() {
            return Err("memory.database_url_env must not be empty".into());
        }
        if self.memory.required && std::env::var_os(&self.memory.database_url_env).is_none() {
            return Err(format!(
                "required persistent memory backend is unavailable: environment variable {} is not set",
                self.memory.database_url_env
            ));
        }
        self.proxy
            .auth_bind
            .parse::<std::net::SocketAddr>()
            .map_err(|e| format!("invalid proxy.auth_bind: {e}"))?;
        self.proxy
            .world_bind
            .parse::<std::net::SocketAddr>()
            .map_err(|e| format!("invalid proxy.world_bind: {e}"))?;
        self.proxy
            .transparent_world_bind
            .parse::<std::net::SocketAddr>()
            .map_err(|e| format!("invalid proxy.transparent_world_bind: {e}"))?;
        if self.proxy.max_pre_auth_connections == 0
            || self.proxy.max_pre_auth_connections_per_ip == 0
        {
            return Err("proxy pre-authentication limits must be positive".into());
        }
        if self.proxy.auth_bind == self.proxy.world_bind
            || self.proxy.auth_bind == self.proxy.transparent_world_bind
            || self.proxy.world_bind == self.proxy.transparent_world_bind
        {
            return Err("proxy listener addresses must be distinct".into());
        }
        if self.upstream.realm_name.trim().is_empty()
            || self.upstream.auth_host.trim().is_empty()
            || self.upstream.world_host.trim().is_empty()
        {
            return Err("upstream auth_host, world_host, and realm_name are required".into());
        }
        let mut lanes = std::collections::BTreeSet::new();
        let mut names = std::collections::BTreeSet::new();
        for account in &self.accounts {
            if !lanes.insert(account.lane) {
                return Err(format!("duplicate lane {}", account.lane));
            }
            let upper = account.account_name.to_uppercase();
            if !names.insert(upper.clone()) {
                return Err(format!("duplicate account {upper}"));
            }
            if account.password.is_empty() {
                return Err(format!("password is empty for account {upper}"));
            }
        }
        Ok(())
    }
}
