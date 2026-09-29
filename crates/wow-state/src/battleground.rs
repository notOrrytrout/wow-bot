use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The status reported for one client battleground queue slot.
///
/// Unknown values remain available so newer server states are not confused
/// with an empty queue slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BattlegroundQueueStatus {
    None,
    WaitQueue,
    WaitJoin,
    InProgress,
    WaitLeave,
    Unknown(u32),
}

impl BattlegroundQueueStatus {
    pub fn from_raw(value: u32) -> Self {
        match value {
            0 => Self::None,
            1 => Self::WaitQueue,
            2 => Self::WaitJoin,
            3 => Self::InProgress,
            4 => Self::WaitLeave,
            value => Self::Unknown(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BattlegroundQueueState {
    pub queue_slot: u32,
    pub battleground_type_id: Option<u32>,
    pub status: BattlegroundQueueStatus,
    pub map_id: Option<u32>,
    pub invite_timeout_ms: Option<u32>,
    pub elapsed_time_ms: Option<u32>,
    pub auto_leave_time_ms: Option<u32>,
    pub team_alliance: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BattlegroundState {
    /// Latest status by client queue slot. A `None` status removes its slot.
    #[serde(default)]
    pub queues: BTreeMap<u32, BattlegroundQueueState>,
}

#[cfg(test)]
mod tests {
    use super::BattlegroundQueueStatus;

    #[test]
    fn known_wire_statuses_are_typed_and_unknown_values_are_retained() {
        assert_eq!(
            BattlegroundQueueStatus::from_raw(0),
            BattlegroundQueueStatus::None
        );
        assert_eq!(
            BattlegroundQueueStatus::from_raw(1),
            BattlegroundQueueStatus::WaitQueue
        );
        assert_eq!(
            BattlegroundQueueStatus::from_raw(2),
            BattlegroundQueueStatus::WaitJoin
        );
        assert_eq!(
            BattlegroundQueueStatus::from_raw(3),
            BattlegroundQueueStatus::InProgress
        );
        assert_eq!(
            BattlegroundQueueStatus::from_raw(4),
            BattlegroundQueueStatus::WaitLeave
        );
        assert_eq!(
            BattlegroundQueueStatus::from_raw(0xffff),
            BattlegroundQueueStatus::Unknown(0xffff)
        );
    }

    #[test]
    fn unknown_status_survives_observation_serialization() {
        let observation =
            crate::ProtocolObservation::BattlegroundQueue(super::BattlegroundQueueState {
                queue_slot: 2,
                battleground_type_id: Some(32),
                status: BattlegroundQueueStatus::Unknown(99),
                map_id: None,
                invite_timeout_ms: None,
                elapsed_time_ms: None,
                auto_leave_time_ms: None,
                team_alliance: None,
            });

        let encoded = serde_json::to_string(&observation).expect("serialize observation");
        let decoded: crate::ProtocolObservation =
            serde_json::from_str(&encoded).expect("deserialize observation");
        let crate::ProtocolObservation::BattlegroundQueue(queue) = decoded else {
            panic!("decoded the wrong observation variant");
        };
        assert_eq!(queue.status, BattlegroundQueueStatus::Unknown(99));
    }
}
