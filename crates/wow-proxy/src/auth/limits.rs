use std::{collections::HashMap, net::IpAddr, sync::{Arc, Mutex}};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy, Debug)]
pub struct AdmissionLimits { pub max_total: usize, pub max_per_ip: usize }
impl Default for AdmissionLimits { fn default() -> Self { Self { max_total: 128, max_per_ip: 16 } } }

#[derive(Clone)]
pub struct AdmissionController {
    global: Arc<Semaphore>,
    per_ip_limit: usize,
    per_ip: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

pub struct AdmissionPermit {
    _global: OwnedSemaphorePermit,
    ip: IpAddr,
    per_ip: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl AdmissionController {
    pub fn new(limits: AdmissionLimits) -> Self {
        Self { global: Arc::new(Semaphore::new(limits.max_total)), per_ip_limit: limits.max_per_ip, per_ip: Arc::new(Mutex::new(HashMap::new())) }
    }
    pub fn try_acquire(&self, ip: IpAddr) -> Result<AdmissionPermit, &'static str> {
        let global = self.global.clone().try_acquire_owned().map_err(|_| "global pre-authentication limit reached")?;
        let mut counts = self.per_ip.lock().map_err(|_| "admission state poisoned")?;
        let count = counts.entry(ip).or_insert(0);
        if *count >= self.per_ip_limit { drop(global); return Err("per-IP pre-authentication limit reached"); }
        *count += 1;
        drop(counts);
        Ok(AdmissionPermit { _global: global, ip, per_ip: self.per_ip.clone() })
    }
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        if let Ok(mut counts) = self.per_ip.lock() {
            if let Some(value) = counts.get_mut(&self.ip) {
                *value = value.saturating_sub(1);
                if *value == 0 { counts.remove(&self.ip); }
            }
        }
    }
}
