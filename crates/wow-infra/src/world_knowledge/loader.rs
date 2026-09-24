use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldKnowledge {
    pub version: String,
    pub payload: serde_json::Value,
}
pub fn load(p: impl AsRef<Path>) -> anyhow::Result<WorldKnowledge> {
    Ok(serde_json::from_slice(&fs::read(p)?)?)
}
