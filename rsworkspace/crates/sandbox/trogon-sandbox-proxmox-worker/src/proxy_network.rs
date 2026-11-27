use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

use crate::{AntiSpoof, ProxyNetworkError};

/// The isolated bridge guests live on and the egress proxy they reach through it.
#[derive(Clone)]
pub struct ProxyNetwork {
    pub bridge: String,
    pub addresses: Vec<Ipv4Net>,
    pub gateway: Ipv4Addr,
    pub bind_address: Ipv4Addr,
    pub advertised_address: Ipv4Addr,
    /// What to do when Proxmox does not report the anti-spoofing filters as active.
    pub antispoof: AntiSpoof,
}

impl ProxyNetwork {
    /// Checks the pool covers `capacity` leases and every address can really be handed to a guest.
    pub fn validate(&self, capacity: usize) -> Result<(), ProxyNetworkError> {
        let bridge_valid = !self.bridge.is_empty()
            && self
                .bridge
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if !bridge_valid {
            return Err(ProxyNetworkError::Bridge);
        }
        if self.addresses.len() < capacity {
            return Err(ProxyNetworkError::PoolTooSmall);
        }
        let mut seen = std::collections::HashSet::new();
        for address in &self.addresses {
            let usable = address.contains(&self.gateway)
                && address.addr() != self.gateway
                && address.addr() != address.network()
                && address.addr() != address.broadcast();
            if !usable {
                return Err(ProxyNetworkError::Address(address.addr()));
            }
            if !seen.insert(address.addr()) {
                return Err(ProxyNetworkError::Duplicate(address.addr()));
            }
        }
        if self.advertised_address.is_unspecified() || self.advertised_address.is_loopback() {
            return Err(ProxyNetworkError::Advertised);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
