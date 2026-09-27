use serde::{Deserialize, Serialize};
use wow_domain::WorldPosition;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PositionState {
    pub player: Option<WorldPosition>,
    pub moving: bool,
    pub flags: u32,
    pub client_time: u32,
    #[serde(default)]
    pub run_speed_yards_per_second: Option<f32>,
}
