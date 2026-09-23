use super::decision::ModelDecision;
use std::time::Duration;
use wow_domain::Mission;
use wow_state::SanitizedSnapshot;

#[derive(Clone, Copy, Debug)]
pub struct ProviderBounds { pub total_timeout: Duration, pub max_response_bytes: usize }
impl Default for ProviderBounds { fn default() -> Self { Self { total_timeout: Duration::from_secs(15), max_response_bytes: 64 * 1024 } } }

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError { Timeout, Transport(String), TooLarge, InvalidUtf8, InvalidJson(String), Http(u16) }

pub trait ModelProvider: Send + Sync {
    fn decide(&self, mission: &Mission, state: &SanitizedSnapshot) -> Result<ModelDecision, ProviderError>;
}

pub fn validate_response(status: u16, body: &[u8], bounds: ProviderBounds) -> Result<&str, ProviderError> {
    if !(200..300).contains(&status) { return Err(ProviderError::Http(status)); }
    if body.len() > bounds.max_response_bytes { return Err(ProviderError::TooLarge); }
    std::str::from_utf8(body).map_err(|_| ProviderError::InvalidUtf8)
}
