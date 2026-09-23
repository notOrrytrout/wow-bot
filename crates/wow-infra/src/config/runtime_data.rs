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
            vmaps: self.vmaps.clone().unwrap_or_else(|| self.root.join("vmaps")),
            mmaps: self.mmaps.clone().unwrap_or_else(|| self.root.join("mmaps")),
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
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            data_dir: "data".into(),
            control_bind: "127.0.0.1:7878".into(),
            worker_queue: 256,
            handshake_timeout_ms: 10_000,
            runtime_data: RuntimeDataPaths { root: "data".into(), dbc: None, maps: None, vmaps: None, mmaps: None },
        }
    }
}
