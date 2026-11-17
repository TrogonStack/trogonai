use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::vec;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::task::JoinHandle;

/// Resolves host names the way reqwest's default resolver does (hyper-util's `GaiResolver`): one blocking
/// `getaddrinfo` per name on tokio's blocking pool, the answer in the order the system returns it, and a cancelled
/// lookup reported as an [`io::ErrorKind::Interrupted`] error. The only difference is that every failure reaches the
/// caller as a [`NameLookupError`], which is how a transport failure is told apart as a lookup that found no host.
///
/// [`crate::BridgeClient::native`] installs it. A caller that builds its own `reqwest::Client` installs it with
/// `reqwest::ClientBuilder::dns_resolver` to get the same error codes.
#[derive(Clone, Copy, Debug, Default)]
pub struct GetAddrInfoResolver;

/// A host name that could not be turned into addresses: the failure Node reports as `getaddrinfo ENOTFOUND`.
///
/// A custom `reqwest::dns::Resolve` reports its failures as this error to have them read as a missing host.
#[derive(Debug, thiserror::Error)]
#[error("host name lookup failed")]
pub struct NameLookupError {
    #[source]
    source: io::Error,
}

impl NameLookupError {
    pub fn new(source: io::Error) -> Self {
        Self { source }
    }

    /// The lookup failure as the system reported it.
    pub fn io_error(&self) -> &io::Error {
        &self.source
    }
}

impl Resolve for GetAddrInfoResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let lookup = Lookup(tokio::task::spawn_blocking(move || {
            (host.as_str(), 0).to_socket_addrs()
        }));
        Box::pin(async move {
            let addrs = lookup.answer().await.map_err(NameLookupError::new)?;
            Ok(Box::new(addrs) as Addrs)
        })
    }
}

/// The blocking lookup, aborted when the request that asked for it is dropped, as `GaiFuture` does.
struct Lookup(JoinHandle<io::Result<vec::IntoIter<SocketAddr>>>);

impl Lookup {
    async fn answer(mut self) -> io::Result<vec::IntoIter<SocketAddr>> {
        match (&mut self.0).await {
            Ok(answer) => answer,
            Err(failure) => match failure.try_into_panic() {
                Ok(payload) => std::panic::resume_unwind(payload),
                Err(cancelled) => Err(io::Error::new(io::ErrorKind::Interrupted, cancelled)),
            },
        }
    }
}

impl Drop for Lookup {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
#[path = "get_addr_info_resolver_tests.rs"]
mod tests;
