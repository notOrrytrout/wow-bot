use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub enum PlayerEvent {
    Attached { connection: u64 },
    Detached { connection: u64 },
    Moved { at: Instant },
    ChannelActivity { until: Instant },
    Command(String),
}

#[derive(Clone, Debug)]
pub struct PlayerPresence {
    connections: BTreeSet<u64>,
    pub last_movement: Option<Instant>,
    pub channel_block_until: Option<Instant>,
    pub idle_resume_armed: bool,
    pub explicit_manual_off: bool,
    pub idle_window: Duration,
    resume_retry_at: Option<Instant>,
    resume_retry_delay: Duration,
}
impl PlayerPresence {
    pub fn new(idle_window: Duration) -> Self {
        Self {
            connections: BTreeSet::new(),
            last_movement: None,
            channel_block_until: None,
            idle_resume_armed: false,
            explicit_manual_off: false,
            idle_window,
            resume_retry_at: None,
            resume_retry_delay: Duration::from_secs(1),
        }
    }
    pub fn attach(&mut self, id: u64) {
        self.connections.insert(id);
    }
    pub fn detach(&mut self, id: u64) -> bool {
        self.connections.remove(&id);
        self.connections.is_empty()
    }
    pub fn count(&self) -> usize {
        self.connections.len()
    }
    pub fn moved(&mut self, now: Instant) {
        self.last_movement = Some(now);
        self.reset_resume_retry();
        if !self.explicit_manual_off {
            self.idle_resume_armed = true;
        }
    }
    pub fn bot_on(&mut self) {
        self.explicit_manual_off = false;
        self.idle_resume_armed = true;
        self.reset_resume_retry();
    }
    pub fn bot_off(&mut self) {
        self.explicit_manual_off = true;
        self.idle_resume_armed = false;
    }
    pub fn block_channel_until(&mut self, until: Instant) {
        self.channel_block_until =
            Some(self.channel_block_until.map_or(until, |old| old.max(until)));
    }
    pub fn resume_deadline(&self) -> Option<Instant> {
        if !self.idle_resume_armed || self.explicit_manual_off {
            return None;
        }
        let movement = self.last_movement.map(|t| t + self.idle_window)?;
        Some(
            self.channel_block_until
                .map_or(movement, |c| c.max(movement)),
        )
    }
    pub fn should_resume(&self, now: Instant) -> bool {
        self.resume_deadline().is_some_and(|deadline| {
            now >= deadline && self.resume_retry_at.is_none_or(|retry_at| now >= retry_at)
        })
    }
    pub fn resume_failed(&mut self, now: Instant) {
        self.resume_retry_at = Some(now + self.resume_retry_delay);
        self.resume_retry_delay = (self.resume_retry_delay * 2).min(Duration::from_secs(30));
    }
    pub fn resumed(&mut self) {
        self.idle_resume_armed = false;
        self.channel_block_until = None;
        self.reset_resume_retry();
    }
    fn reset_resume_retry(&mut self) {
        self.resume_retry_at = None;
        self.resume_retry_delay = Duration::from_secs(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_idle_resume_uses_virtual_time_and_caps_retry_backoff() {
        let started = Instant::now();
        let mut player = PlayerPresence::new(Duration::from_secs(2));
        player.bot_on();
        player.moved(started);
        let mut failure_at = started + player.idle_window;

        for delay in [1, 2, 4, 8, 16, 30, 30].map(Duration::from_secs) {
            assert!(player.should_resume(failure_at));
            player.resume_failed(failure_at);
            let retry_at = failure_at + delay;
            assert!(!player.should_resume(retry_at - Duration::from_millis(1)));
            assert!(player.should_resume(retry_at));
            failure_at = retry_at;
        }
    }

    #[test]
    fn new_player_movement_resets_resume_retry_delay() {
        let started = Instant::now();
        let mut player = PlayerPresence::new(Duration::from_secs(2));
        player.bot_on();
        player.moved(started);
        let idle_deadline = started + player.idle_window;
        player.resume_failed(idle_deadline);
        player.moved(idle_deadline + Duration::from_secs(3));

        let next_deadline = idle_deadline + Duration::from_secs(3) + player.idle_window;
        assert!(player.should_resume(next_deadline));
        player.resume_failed(next_deadline);
        assert!(!player.should_resume(next_deadline + Duration::from_millis(999)));
        assert!(player.should_resume(next_deadline + Duration::from_secs(1)));
    }
}
