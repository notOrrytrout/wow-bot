use wow_domain::{Mission, MissionIntent, StrategicIntent};
use wow_state::Snapshot;

pub trait MissionPolicy { fn choose(&self, mission: &Mission, state: &Snapshot) -> StrategicIntent; }

pub fn exact_name_scope<'a>(mission: &'a Mission) -> Option<&'a str> {
    match &mission.intent {
        MissionIntent::Gather { resource } => Some(resource),
        MissionIntent::Grind { creature } => Some(creature),
        _ => None,
    }
}

pub fn normalize_scope(value: &str) -> String { value.trim().to_lowercase() }
