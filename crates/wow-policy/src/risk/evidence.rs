use std::time::{Duration, Instant};
#[derive(Clone, Debug)]
pub struct RiskEvidence {
    pub source: String,
    pub score: f32,
    pub observed_at: Instant,
    pub ttl: Duration,
}
impl RiskEvidence {
    pub fn current(&self, now: Instant) -> bool {
        now.duration_since(self.observed_at) <= self.ttl
    }
}
pub fn bounded_current<'a>(
    evidence: &'a [RiskEvidence],
    now: Instant,
    max: usize,
) -> Vec<&'a RiskEvidence> {
    evidence
        .iter()
        .rev()
        .filter(|e| e.current(now))
        .take(max)
        .collect()
}
