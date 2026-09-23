use tentacli::plugins::wow::wotlk::realm::object::types::movement::Point3D;
use wow_domain::{Vec3, WorldPosition};
use wow_state::ProtocolObservation;

pub fn point(point: Point3D, map: u32, orientation: f32) -> WorldPosition {
    WorldPosition { map, point: Vec3::new(point.x, point.y, point.z), orientation }
}

pub fn player_position(position: WorldPosition, moving: bool, flags: u32, client_time: u32) -> ProtocolObservation {
    ProtocolObservation::PlayerPosition { position, moving, flags, client_time }
}
