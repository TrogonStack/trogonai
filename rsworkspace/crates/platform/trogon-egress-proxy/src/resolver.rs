//! Name resolution, behind a trait so tests (and a future pinned resolver) can
//! stand in for the system resolver. The caller resolves a name once per
//! tunnel and never again (ADR#0085 Decision 7).

use std::io;
use std::net::IpAddr;

use async_trait::async_trait;

use crate::host_name::HostName;

#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &HostName) -> io::Result<Vec<IpAddr>>;
}

/// The operating system resolver.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &HostName) -> io::Result<Vec<IpAddr>> {
        let addresses = tokio::net::lookup_host((host.as_str(), 0)).await?;
        Ok(addresses.map(|address| address.ip()).collect())
    }
}

#[cfg(test)]
mod tests;
