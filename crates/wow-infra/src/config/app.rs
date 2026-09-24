use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use wow_domain::{AccountId, LaneId};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub lane: LaneId,
    pub account: AccountId,
    pub account_name: String,
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
    3724
}
const fn default_world_port() -> u16 {
    8085
}
fn default_proxy_auth_bind() -> String {
    "0.0.0.0:3725".to_owned()
}
fn default_proxy_world_bind() -> String {
    "0.0.0.0:8086".to_owned()
}
fn default_transparent_world_bind() -> String {
    "0.0.0.0:8088".to_owned()
}
fn default_advertise_host() -> String {
    "127.0.0.1".to_owned()
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
            auth_host: "127.0.0.1".into(),
            auth_port: default_auth_port(),
            world_host: "127.0.0.1".into(),
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

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            setup_complete: false,
            runtime: super::runtime_data::RuntimeConfig::default(),
            upstream: UpstreamConfig::default(),
            proxy: ProxyConfig::default(),
            accounts: Vec::new(),
        }
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
        for account in self.accounts.iter().filter(|a| a.enabled) {
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
