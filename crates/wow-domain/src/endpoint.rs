use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Endpoint { pub host: String, pub port: u16 }

impl Endpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Result<Self, &'static str> {
        let host = host.into();
        if host.trim().is_empty() { return Err("host is empty"); }
        if port == 0 { return Err("port must be non-zero"); }
        Ok(Self { host, port })
    }
    pub fn authority(&self) -> String {
        match self.host.parse::<IpAddr>() {
            Ok(IpAddr::V6(_)) => format!("[{}]:{}", self.host, self.port),
            _ => format!("{}:{}", self.host, self.port),
        }
    }
}

#[cfg(test)]
mod tests { use super::*; #[test] fn ipv6_is_bracketed(){let e=Endpoint::new("::1",8085).unwrap();assert_eq!(e.authority(),"[::1]:8085");} }
