use std::net::IpAddr;

#[derive(Clone, Debug)]
pub struct RemoteMemoryPolicy {
    pub require_verified_tls: bool,
}
impl Default for RemoteMemoryPolicy {
    fn default() -> Self {
        Self {
            require_verified_tls: true,
        }
    }
}

pub fn validate_remote_endpoint(
    host: &str,
    tls: bool,
    verify_identity: bool,
    policy: &RemoteMemoryPolicy,
) -> Result<(), String> {
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if policy.require_verified_tls && !loopback && (!tls || !verify_identity) {
        return Err("remote memory requires verified TLS for non-loopback endpoints".into());
    }
    Ok(())
}

pub trait RemoteMemory: Send + Sync {
    fn fetch(&self, key: &str) -> Result<Option<String>, String>;
    fn store(&self, key: &str, value: &str) -> Result<(), String>;
}
