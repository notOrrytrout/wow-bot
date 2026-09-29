pub mod combat;
pub mod lifecycle;
pub mod objectives;
pub mod vehicles;

/// Return true for WotLK battleground instance maps. Arenas are not included.
pub fn is_wotlk_battleground_map(map_id: u32) -> bool {
    matches!(map_id, 30 | 489 | 529 | 566 | 607 | 628)
}

/// The server reports a positive auto-leave timer after a battleground ends.
pub fn match_is_ending(queue: &wow_state::battleground::BattlegroundQueueState) -> bool {
    queue.status == wow_state::battleground::BattlegroundQueueStatus::InProgress
        && queue
            .auto_leave_time_ms
            .is_some_and(|remaining| remaining > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_state::battleground::{BattlegroundQueueState, BattlegroundQueueStatus};

    #[test]
    fn auto_leave_countdown_marks_active_match_as_ending() {
        let queue = BattlegroundQueueState {
            queue_slot: 0,
            battleground_type_id: Some(3),
            status: BattlegroundQueueStatus::InProgress,
            map_id: Some(489),
            invite_timeout_ms: None,
            elapsed_time_ms: None,
            auto_leave_time_ms: Some(10_000),
            team_alliance: Some(true),
        };
        assert!(match_is_ending(&queue));
        assert!(!match_is_ending(&BattlegroundQueueState {
            auto_leave_time_ms: Some(0),
            ..queue.clone()
        }));
        assert!(!match_is_ending(&BattlegroundQueueState {
            status: BattlegroundQueueStatus::WaitLeave,
            ..queue
        }));
    }
}
