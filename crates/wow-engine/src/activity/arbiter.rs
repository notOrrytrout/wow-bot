use super::{ActivityKind, ActivityLease};
use wow_domain::{ActivityGeneration, ActivityId, TaskId};
pub struct ActivityArbiter {
    current: Option<ActivityLease>,
    generation: ActivityGeneration,
    next: u64,
}
impl Default for ActivityArbiter {
    fn default() -> Self {
        Self {
            current: None,
            generation: ActivityGeneration::ZERO,
            next: 1,
        }
    }
}
impl ActivityArbiter {
    pub fn current(&self) -> Option<ActivityLease> {
        self.current
    }
    pub fn acquire(&mut self, owner: TaskId, kind: ActivityKind) -> Option<ActivityLease> {
        if self
            .current
            .is_some_and(|c| c.kind.priority() > kind.priority())
        {
            return None;
        }
        self.generation = self.generation.next();
        let l = ActivityLease {
            id: ActivityId(self.next),
            owner,
            kind,
            generation: self.generation,
        };
        self.next = self.next.wrapping_add(1);
        self.current = Some(l);
        Some(l)
    }
    pub fn release(&mut self, lease: ActivityLease) -> bool {
        if self.current == Some(lease) {
            self.current = None;
            true
        } else {
            false
        }
    }
    pub fn is_current(&self, lease: ActivityLease) -> bool {
        self.current == Some(lease)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn higher_priority_preempts_and_old_lease_is_stale() {
        let mut a = ActivityArbiter::default();
        let low = a.acquire(TaskId(1), ActivityKind::Fishing).unwrap();
        let high = a.acquire(TaskId(2), ActivityKind::Recovery).unwrap();
        assert!(!a.is_current(low));
        assert!(a.is_current(high));
        assert!(!a.release(low));
    }
}
