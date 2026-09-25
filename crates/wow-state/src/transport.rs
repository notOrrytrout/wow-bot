use serde::{Deserialize, Serialize};
use wow_domain::{EntityId, Vec3};

/// Authoritative passenger state from a parsed movement observation.
/// Missing transport identity or relative coordinates stay missing; consumers
/// must not infer them from world positions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TransportState {
    pub attached: Option<bool>,
    pub transport: Option<EntityId>,
    pub relative_position: Option<Vec3>,
    pub relative_orientation: Option<f32>,
    pub transport_time: Option<u32>,
}

impl TransportState {
    pub fn has_authoritative_attachment(&self, transport: EntityId) -> bool {
        self.attached == Some(true)
            && self.transport == Some(transport)
            && self.relative_position.is_some_and(Vec3::is_finite)
            && self.relative_orientation.is_some_and(f32::is_finite)
    }
}
