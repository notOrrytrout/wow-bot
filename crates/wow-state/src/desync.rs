use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesyncState {
    pub suspect: bool,
    pub reasons: Vec<String>,
}
