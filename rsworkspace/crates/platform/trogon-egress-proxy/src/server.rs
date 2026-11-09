//! The accept loop: TCP, then the mTLS handshake, then the caller's identity,
//! then [`Proxy::serve`]. A connection that fails any step is dropped before
//! a single HTTP byte is read from it.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use trogon_workload_identity::{IdentityDriver, IdentityStore, MaterialLease};

use crate::connection::Proxy;
use crate::constants::ACCEPT_ERROR_BACKOFF;
use crate::live_policy::KvPolicyDriver;
use crate::peer_identity::PeerIdentity;
use crate::trust_domain::TrustDomain;
use crate::workload_registration_driver::RegistrationDriver;

pub struct Server {
    listener: TcpListener,
    acceptor: ListenerIdentity,
    proxy: Arc<Proxy>,
    trust_domain: Arc<TrustDomain>,
    policy_driver: Option<KvPolicyDriver>,
    identity_driver: Option<IdentityDriver>,
    registration_driver: Option<RegistrationDriver>,
    readiness: crate::Readiness,
}

#[derive(Clone)]
enum ListenerIdentity {
    Fixed(TlsAcceptor),
    Workload(IdentityStore),
}

struct HandshakeAuthority {
    acceptor: TlsAcceptor,
    lease: Option<MaterialLease>,
}

impl ListenerIdentity {
    fn capture(&self) -> Option<HandshakeAuthority> {
        match self {
            Self::Fixed(acceptor) => Some(HandshakeAuthority {
                acceptor: acceptor.clone(),
                lease: None,
            }),
            Self::Workload(identity) => {
                let lease = identity.material()?;
                Some(HandshakeAuthority {
                    acceptor: TlsAcceptor::from(lease.server_config()),
                    lease: Some(lease),
                })
            }
        }
    }
}

impl Server {
    pub fn readiness(&self) -> crate::Readiness {
        self.readiness.clone()
    }

    pub(crate) fn with_readiness(mut self, readiness: crate::Readiness) -> Self {
        self.readiness = readiness;
        self
    }

    pub fn new(listener: TcpListener, tls: Arc<rustls::ServerConfig>, proxy: Proxy, trust_domain: TrustDomain) -> Self {
        let readiness = crate::Readiness::new(None, None, proxy.audit());
        Self {
            listener,
            acceptor: ListenerIdentity::Fixed(TlsAcceptor::from(tls)),
            proxy: Arc::new(proxy),
            trust_domain: Arc::new(trust_domain),
            policy_driver: None,
            identity_driver: None,
            registration_driver: None,
            readiness,
        }
    }

    pub(crate) fn new_workload(
        listener: TcpListener,
        identity: IdentityStore,
        proxy: Proxy,
        trust_domain: TrustDomain,
        driver: IdentityDriver,
        registration: RegistrationDriver,
    ) -> Self {
        let readiness = crate::Readiness::new(None, None, proxy.audit());
        Self {
            listener,
            acceptor: ListenerIdentity::Workload(identity),
            proxy: Arc::new(proxy),
            trust_domain: Arc::new(trust_domain),
            policy_driver: None,
            identity_driver: Some(driver),
            registration_driver: Some(registration),
            readiness,
        }
    }

    pub(crate) fn with_policy_driver(mut self, driver: Option<KvPolicyDriver>) -> Self {
        self.policy_driver = driver;
        self
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts connections until `shutdown` is cancelled. Tunnels still open
    /// at that point are closed. The audit sink's background work runs for the
    /// same span. `run` returns only after every write in progress has
    /// settled, even one whose connection closed under it, and the sink has
    /// made its last flush. A record still buffered when that flush fails, as
    /// it does while the stream stays down, is lost with the process.
    pub async fn run(mut self, shutdown: CancellationToken) {
        let _activity = self.readiness.running(shutdown.clone());
        let flush = CancellationToken::new();
        let _flush_on_drop = flush.clone().drop_guard();
        let audit = self.proxy.audit();
        let drain = tokio::spawn({
            let flush = flush.clone();
            async move { audit.run(flush).await }
        });
        let policy_shutdown = CancellationToken::new();
        let policy = self.policy_driver.take();
        let identity = self.identity_driver.take();
        let registration = self.registration_driver.take();
        tokio::join!(
            async {
                accept_loop(
                    &self.listener,
                    &self.acceptor,
                    &self.proxy,
                    &self.trust_domain,
                    shutdown,
                )
                .await;
                policy_shutdown.cancel();
            },
            async {
                if let Some(policy) = policy {
                    policy.run(policy_shutdown.clone()).await;
                }
            },
            async {
                if let Some(identity) = identity {
                    identity.run(policy_shutdown.clone()).await;
                }
            },
            async {
                if let Some(registration) = registration {
                    registration.run(policy_shutdown.clone()).await;
                }
            }
        );
        flush.cancel();
        if let Err(error) = drain.await {
            tracing::warn!(error = %error, "audit drain task ended abnormally");
        }
    }
}

/// Where connections come from. A seam so a test can make `accept` fail, which
/// a real listener only does when the process is out of descriptors.
trait Accept {
    fn accept(&self) -> impl Future<Output = io::Result<(TcpStream, SocketAddr)>> + Send;
}

impl Accept for TcpListener {
    fn accept(&self) -> impl Future<Output = io::Result<(TcpStream, SocketAddr)>> + Send {
        TcpListener::accept(self)
    }
}

async fn accept_loop<L: Accept>(
    listener: &L,
    acceptor: &ListenerIdentity,
    proxy: &Arc<Proxy>,
    trust_domain: &Arc<TrustDomain>,
    shutdown: CancellationToken,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    while connections.try_join_next().is_some() {}
                    if connections.len() >= proxy.limits().max_connections.get() {
                        drop(stream);
                        continue;
                    }
                    connections.spawn(serve_connection(
                        acceptor.clone(),
                        Arc::clone(proxy),
                        Arc::clone(trust_domain),
                        stream,
                        peer,
                    ));
                }
                Err(error) => {
                    tracing::warn!(error = %error, "accept failed");
                    tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                }
            },
        }
    }
    connections.shutdown().await;
}

async fn serve_connection(
    source: ListenerIdentity,
    proxy: Arc<Proxy>,
    trust_domain: Arc<TrustDomain>,
    stream: TcpStream,
    peer: SocketAddr,
) {
    let Some(HandshakeAuthority { acceptor, lease }) = source.capture() else {
        return;
    };
    let handshake = match &lease {
        Some(lease) => tokio::select! {
            biased;
            () = lease.changed() => return,
            handshake = timeout(proxy.limits().handshake_timeout, acceptor.accept(stream)) => handshake,
        },
        None => timeout(proxy.limits().handshake_timeout, acceptor.accept(stream)).await,
    };
    let tls = match handshake {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            tracing::warn!(%peer, error = %error, "TLS handshake refused");
            return;
        }
        Err(_) => {
            tracing::warn!(%peer, "TLS handshake timed out");
            return;
        }
    };
    let certificates = tls.get_ref().1.peer_certificates().unwrap_or_default();
    let identity = match lease {
        Some(lease) => match PeerIdentity::from_workload_certificates(certificates, lease) {
            Ok(identity) => identity,
            Err(error) => {
                tracing::warn!(%peer, error = %error, "workload peer refused");
                return;
            }
        },
        None => match PeerIdentity::from_verified_certificates(certificates, &trust_domain) {
            Ok(identity) => identity,
            Err(error) => {
                tracing::warn!(%peer, error = %error, "client certificate names no agent");
                return;
            }
        },
    };
    proxy.serve(tls, &identity).await;
}

#[cfg(test)]
mod tests;
