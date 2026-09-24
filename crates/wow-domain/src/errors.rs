use thiserror::Error;
#[derive(Debug, Error)]
pub enum DomainError {
    #[error("stale generation: {0}")]
    Stale(&'static str),
    #[error("permission denied: {0}")]
    Permission(&'static str),
    #[error("invalid state: {0}")]
    Invalid(&'static str),
    #[error("queue saturated")]
    Saturated,
    #[error("closed channel")]
    Closed,
}
