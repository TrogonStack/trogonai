//! The address pool host-links networking draws lease links from.

use anyhow::ensure;
use ipnet::Ipv4Net;

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub host_links: Ipv4Net,
}

impl NetworkConfig {
    pub fn validate(&self, capacity: u32) -> anyhow::Result<()> {
        ensure!(
            self.host_links.prefix_len() <= 28 && self.host_links.addr() == self.host_links.network(),
            "host link pool must be an aligned IPv4 network with prefix <= 28"
        );
        ensure!(
            self.host_links.network().is_private() && self.host_links.broadcast().is_private(),
            "host link pool must use private IPv4 addresses"
        );
        ensure!(
            capacity > 0 && u64::from(capacity) <= (1u64 << (30 - self.host_links.prefix_len())),
            "host link pool is smaller than worker capacity"
        );
        Ok(())
    }
}
