use super::runtime_data::{RuntimeConfig, RuntimeDataPaths};
use std::{fs, path::Path};

pub fn validate(config: &RuntimeConfig) -> Result<(), String> {
    if config.worker_queue == 0 { return Err("worker_queue must be positive".into()); }
    if config.handshake_timeout_ms == 0 { return Err("handshake_timeout_ms must be positive".into()); }
    if config.control_bind.trim().is_empty() { return Err("control_bind is required".into()); }
    config.control_bind.parse::<std::net::SocketAddr>().map_err(|e| format!("invalid control_bind: {e}"))?;
    Ok(())
}

pub fn validate_runtime_data(config: &RuntimeConfig) -> Result<(), String> {
    let p = config.runtime_data.resolved();
    for (kind, path) in [("dbc", p.dbc), ("maps", p.maps), ("vmaps", p.vmaps), ("mmaps", p.mmaps)] {
        validate_data_directory(kind, &path)?;
    }
    Ok(())
}

pub fn validate_runtime_data_root(root: impl AsRef<Path>) -> Result<(), String> {
    let paths = RuntimeDataPaths {
        root: root.as_ref().to_path_buf(),
        dbc: None,
        maps: None,
        vmaps: None,
        mmaps: None,
    };
    let resolved = paths.resolved();
    for (kind, path) in [
        ("dbc", resolved.dbc),
        ("maps", resolved.maps),
        ("vmaps", resolved.vmaps),
        ("mmaps", resolved.mmaps),
    ] {
        validate_data_directory(kind, &path)?;
    }
    Ok(())
}

fn validate_data_directory(kind: &str, path: &Path) -> Result<(), String> {
    let md = fs::metadata(path)
        .map_err(|e| format!("missing required {kind} data at {}: {e}", path.display()))?;
    if !md.is_dir() {
        return Err(format!("required {kind} data path is not a directory: {}", path.display()));
    }
    let mut entries = fs::read_dir(path)
        .map_err(|e| format!("failed to read required {kind} data at {}: {e}", path.display()))?;
    if entries.next().is_none() {
        return Err(format!("required {kind} data directory is empty: {}", path.display()));
    }
    Ok(())
}
