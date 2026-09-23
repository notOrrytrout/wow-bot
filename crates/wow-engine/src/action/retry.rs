use wow_domain::ActionFailure;

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy { pub max_attempts: u8, pub backoff_ms: u64 }
impl Default for RetryPolicy { fn default() -> Self { Self { max_attempts: 3, backoff_ms: 250 } } }
impl RetryPolicy {
    pub fn should_retry(self, failure: &ActionFailure, attempts: u8) -> bool {
        failure.retryable && attempts < self.max_attempts
    }
    pub fn delay_ms(self, attempts: u8) -> u64 {
        self.backoff_ms.saturating_mul(1_u64 << attempts.min(8))
    }
}
