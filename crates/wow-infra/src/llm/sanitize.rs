use serde::Serialize;
use wow_state::{SanitizedSnapshot, Snapshot};

#[derive(Clone, Copy, Debug)]
pub struct ContextPolicy { pub max_json_bytes: usize }
impl Default for ContextPolicy { fn default() -> Self { Self { max_json_bytes: 32 * 1024 } } }

pub fn snapshot(state: &Snapshot) -> SanitizedSnapshot { state.into() }

pub fn bounded_json<T: Serialize>(value: &T, policy: ContextPolicy) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > policy.max_json_bytes { return Err(format!("model context exceeds {} bytes", policy.max_json_bytes)); }
    Ok(bytes)
}
