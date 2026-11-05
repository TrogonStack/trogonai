//! Startup: policy first, then TLS, then the listener. Any failure is a typed
//! error and the process does not serve (ADR#0084: enforce by default and
//! refuse to start with no policy).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::audit::{AuditSink, TracingAudit};
use crate::config::Config;
use crate::connection::Proxy;
use crate::mtls::{TlsError, load_server_config};
use crate::policy_snapshot::PolicySnapshot;
use crate::resolver::{Resolver, SystemResolver};
use crate::server::Server;
use crate::static_snapshot::{SnapshotError, StaticSnapshot};

/// What the proxy does with a request the policy does not permit. Enforcement
/// is the only mode; the startup line states it so a log never has to be
/// interpreted to learn that the proxy is protecting anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementMode {
    Enforce,
}

impl EnforcementMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("no usable policy, refusing to start")]
    Policy(#[source] SnapshotError),
    #[error("TLS configuration is not usable")]
    Tls(#[source] TlsError),
    #[error("cannot listen on {address}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: io::Error,
    },
}

/// Starts the proxy with the system resolver and the tracing audit sink.
pub async fn start(config: Config) -> Result<Server, StartupError> {
    start_with(config, Arc::new(SystemResolver), Arc::new(TracingAudit)).await
}

pub async fn start_with(
    config: Config,
    resolver: Arc<dyn Resolver>,
    audit: Arc<dyn AuditSink>,
) -> Result<Server, StartupError> {
    let snapshot = StaticSnapshot::load(&config.policy_file).map_err(StartupError::Policy)?;
    let tls = load_server_config(&config.tls, config.trust_domain.clone()).map_err(StartupError::Tls)?;
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|source| StartupError::Bind {
            address: config.listen,
            source,
        })?;

    let mode = EnforcementMode::Enforce;
    tracing::info!(
        policy_source = %snapshot.source(),
        policy_kind = snapshot.source().kind(),
        mode = mode.as_str(),
        agents = snapshot.agent_count(),
        permits = snapshot.permit_count(),
        listen = %config.listen,
        trust_domain = %config.trust_domain,
        "egress proxy starting"
    );

    let proxy = Proxy::new(
        Arc::new(snapshot),
        resolver,
        audit,
        config.limits,
        config.non_tls_ports.into_iter().collect(),
    );
    Ok(Server::new(listener, Arc::new(tls), proxy, config.trust_domain))
}

#[cfg(test)]
mod tests;
