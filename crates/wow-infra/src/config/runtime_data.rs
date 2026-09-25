use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDataPaths {
    pub root: PathBuf,
    pub dbc: Option<PathBuf>,
    pub maps: Option<PathBuf>,
    pub vmaps: Option<PathBuf>,
    pub mmaps: Option<PathBuf>,
}

impl RuntimeDataPaths {
    pub fn resolved(&self) -> ResolvedRuntimeDataPaths {
        ResolvedRuntimeDataPaths {
            dbc: self.dbc.clone().unwrap_or_else(|| self.root.join("dbc")),
            maps: self.maps.clone().unwrap_or_else(|| self.root.join("maps")),
            vmaps: self
                .vmaps
                .clone()
                .unwrap_or_else(|| self.root.join("vmaps")),
            mmaps: self
                .mmaps
                .clone()
                .unwrap_or_else(|| self.root.join("mmaps")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedRuntimeDataPaths {
    pub dbc: PathBuf,
    pub maps: PathBuf,
    pub vmaps: PathBuf,
    pub mmaps: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub data_dir: PathBuf,
    pub control_bind: String,
    pub worker_queue: usize,
    pub handshake_timeout_ms: u64,
    pub runtime_data: RuntimeDataPaths,
    #[serde(default)]
    pub runtime_tuning: RuntimeTuning,
}

/// Settings that tune autonomous movement and maintenance behavior.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeTuning {
    pub movement: MovementTuning,
    pub maintenance: MaintenanceTuning,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MovementTuning {
    pub travel_speed_form_min_yards: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MaintenanceTuning {
    /// Disabled by default to preserve the behavior of existing JSON configs.
    pub auto_mount_enabled: bool,
    pub mount_min_travel_yards: u32,
}

impl Default for RuntimeTuning {
    fn default() -> Self {
        Self {
            movement: MovementTuning::default(),
            maintenance: MaintenanceTuning::default(),
        }
    }
}

impl Default for MovementTuning {
    fn default() -> Self {
        Self {
            travel_speed_form_min_yards: 30,
        }
    }
}

impl Default for MaintenanceTuning {
    fn default() -> Self {
        Self {
            auto_mount_enabled: false,
            mount_min_travel_yards: 80,
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            data_dir: "data".into(),
            control_bind: "127.0.0.1:7878".into(),
            worker_queue: 256,
            handshake_timeout_ms: 10_000,
            runtime_data: RuntimeDataPaths {
                root: "data".into(),
                dbc: None,
                maps: None,
                vmaps: None,
                mmaps: None,
            },
            runtime_tuning: RuntimeTuning::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_json_runtime_config_keeps_auto_mount_disabled() {
        let config: RuntimeConfig = serde_json::from_str(
            r#"{
                "data_dir":"data",
                "control_bind":"127.0.0.1:7878",
                "worker_queue":256,
                "handshake_timeout_ms":10000,
                "runtime_data":{"root":"data","dbc":null,"maps":null,"vmaps":null,"mmaps":null}
            }"#,
        )
        .unwrap();
        assert_eq!(
            config.runtime_tuning.movement.travel_speed_form_min_yards,
            30
        );
        assert!(!config.runtime_tuning.maintenance.auto_mount_enabled);
        assert_eq!(config.runtime_tuning.maintenance.mount_min_travel_yards, 80);
    }
}
