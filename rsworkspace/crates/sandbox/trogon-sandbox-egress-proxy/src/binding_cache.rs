//! A short-lived record of which addresses a name resolved to, built only from
//! lookups this proxy performed itself.
//!
//! What a sandboxed client resolved, or claims to have resolved, is unknowable
//! here and may be forged, so it is never an input. A domain-based allow is
//! attributed to a destination address only when this cache holds a live
//! (name, address) pair. Keying on the pair, rather than the address, keeps
//! two names that share an edge address apart: a binding for one says nothing
//! about the other.

use crate::host_kind::{HostClassification, HostKind};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use trogon_sandbox_contract::Host;

use crate::fold_mapped::fold_mapped;

mod constants;
pub use constants::DEFAULT_BINDING_TTL;
use constants::MAX_BINDINGS;

type Key = (String, IpAddr);

pub struct BindingCache {
    ttl: Duration,
    bindings: Mutex<HashMap<Key, Instant>>,
}

impl Default for BindingCache {
    fn default() -> Self {
        Self::new(DEFAULT_BINDING_TTL)
    }
}

impl BindingCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            bindings: Mutex::new(HashMap::new()),
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Records that the proxy's own lookup of `name` answered `addr`. An IP
    /// literal names itself and is not a DNS observation, so it is ignored.
    pub fn record(&self, name: &Host, addr: IpAddr) {
        self.record_at(name, addr, Instant::now());
    }

    pub fn record_at(&self, name: &Host, addr: IpAddr, now: Instant) {
        let HostKind::Domain(domain) = name.kind() else { return };
        let mut bindings = self.lock();
        bindings.insert((domain.as_str().to_string(), fold_mapped(addr)), now);
        if bindings.len() > MAX_BINDINGS {
            let ttl = self.ttl;
            bindings.retain(|_, seen| now.saturating_duration_since(*seen) < ttl);
            let excess = bindings.len().saturating_sub(MAX_BINDINGS);
            let mut by_age: Vec<_> = bindings.iter().map(|(key, seen)| (*seen, key.clone())).collect();
            by_age.sort();
            for (_, key) in by_age.into_iter().take(excess) {
                bindings.remove(&key);
            }
        }
    }

    /// Whether the proxy observed `name` resolving to `addr` within the TTL.
    pub fn is_bound(&self, name: &Host, addr: IpAddr) -> bool {
        self.is_bound_at(name, addr, Instant::now())
    }

    pub fn is_bound_at(&self, name: &Host, addr: IpAddr, now: Instant) -> bool {
        let HostKind::Domain(domain) = name.kind() else {
            return false;
        };
        let key = (domain.as_str().to_string(), fold_mapped(addr));
        self.lock()
            .get(&key)
            .is_some_and(|seen| now.saturating_duration_since(*seen) < self.ttl)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Instant>> {
        self.bindings.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
