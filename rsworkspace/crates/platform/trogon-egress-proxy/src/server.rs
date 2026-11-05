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

use crate::connection::Proxy;
use crate::constants::ACCEPT_ERROR_BACKOFF;
use crate::mtls::peer_agent;
use crate::trust_domain::TrustDomain;

pub struct Server {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    proxy: Arc<Proxy>,
    trust_domain: Arc<TrustDomain>,
}

impl Server {
    pub fn new(listener: TcpListener, tls: Arc<rustls::ServerConfig>, proxy: Proxy, trust_domain: TrustDomain) -> Self {
        Self {
            listener,
            acceptor: TlsAcceptor::from(tls),
            proxy: Arc::new(proxy),
            trust_domain: Arc::new(trust_domain),
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts connections until `shutdown` is cancelled. Tunnels still open
    /// at that point are closed. The audit sink's background work runs for the
    /// same span, and `run` returns only after it has flushed, so a decision
    /// made just before shutdown is not lost with the process.
    pub async fn run(self, shutdown: CancellationToken) {
        let flush = CancellationToken::new();
        let audit = self.proxy.audit();
        let drain = tokio::spawn({
            let flush = flush.clone();
            async move { audit.run(flush).await }
        });
        accept_loop(
            &self.listener,
            &self.acceptor,
            &self.proxy,
            &self.trust_domain,
            shutdown,
        )
        .await;
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
    acceptor: &TlsAcceptor,
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
    acceptor: TlsAcceptor,
    proxy: Arc<Proxy>,
    trust_domain: Arc<TrustDomain>,
    stream: TcpStream,
    peer: SocketAddr,
) {
    let handshake = timeout(proxy.limits().handshake_timeout, acceptor.accept(stream)).await;
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
    let agent = match peer_agent(tls.get_ref().1.peer_certificates(), &trust_domain) {
        Ok(agent) => agent,
        Err(error) => {
            tracing::warn!(%peer, error = %error, "client certificate names no agent");
            return;
        }
    };
    proxy.serve(tls, &agent).await;
}

#[cfg(test)]
mod tests;
