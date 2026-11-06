//! Startup: policy first, then TLS, then the listener. Any failure is a typed
//! error and the process does not serve (ADR#0084: enforce by default and
//! refuse to start with no policy). Audit is part of the same bargain
//! (ADR#0085 Decision 5): settings that cannot build an audit writer stop the
//! process. A stream that is configured but not answering does not: NATS
//! connects in the background, so the proxy keeps denying and refuses the
//! tunnels it cannot record until the stream returns.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use trogon_audit::{AuditWriter, JetStreamSink, Stamper, WriterConfig};
use trogon_nats::ConnectError;
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;

use crate::audit::AuditSink;
use crate::audit_config::AuditConfig;
use crate::config::{Config, PolicyInput};
use crate::connection::Proxy;
use crate::constants::AUDIT_CONNECT_TIMEOUT;
use crate::live_policy::{KvPolicyConfig, KvSnapshot, LivePolicyError, PolicyTransportError};
use crate::mtls::{TlsError, load_server_config};
use crate::policy_snapshot::{PolicySnapshot, PolicyStatistics};
use crate::resolver::{Resolver, SystemResolver};
use crate::server::Server;
use crate::static_snapshot::{SnapshotError, StaticSnapshot};
use crate::writer_audit::WriterAudit;

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
    #[error("no usable live policy, refusing to start")]
    LivePolicy(#[source] LivePolicyError),
    #[error("live policy transport is not authenticated, refusing to start")]
    LivePolicyTransport(#[source] PolicyTransportError),
    #[error("live policy scope does not match this runtime")]
    LivePolicyScope,
    #[error("TLS configuration is not usable")]
    Tls(#[source] TlsError),
    #[error("audit connection settings are not usable, refusing to start")]
    Audit(#[source] ConnectError),
    #[error("cannot listen on {address}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: io::Error,
    },
}

/// Starts the proxy with the system resolver and the JetStream audit writer.
pub async fn start(config: Config) -> Result<Server, StartupError> {
    validate_policy_transport(&config)?;
    let audit = connect_audit(&config.audit).await.map_err(StartupError::Audit)?;
    start_with(config, Arc::new(SystemResolver), Arc::new(audit)).await
}

/// The audit writer over the configured stream. The drain task is not spawned
/// here: [`Server::run`] runs it for as long as the server serves.
async fn connect_audit(
    config: &AuditConfig,
) -> Result<WriterAudit<JetStreamSink<async_nats::jetstream::Context>>, ConnectError> {
    let client = trogon_nats::connect(&config.nats, AUDIT_CONNECT_TIMEOUT).await?;
    let sink = JetStreamSink::with_defaults(async_nats::jetstream::new(client));
    let stamper = Stamper::new(SystemClock, UuidV7Generator);
    let writer = AuditWriter::new(sink, config.identity.clone(), WriterConfig::default(), &stamper);
    let identity = &config.identity;
    Ok(WriterAudit::new(
        writer,
        identity.tenant().clone(),
        identity.project().clone(),
    ))
}

pub async fn start_with(
    config: Config,
    resolver: Arc<dyn Resolver>,
    audit: Arc<dyn AuditSink>,
) -> Result<Server, StartupError> {
    validate_policy_transport(&config)?;
    let (snapshot, policy_driver, statistics): (Arc<dyn PolicySnapshot>, _, _) = match &config.policy {
        PolicyInput::StaticFile(path) => {
            let snapshot = StaticSnapshot::load(path).map_err(StartupError::Policy)?;
            let statistics = PolicyStatistics {
                agents: snapshot.agent_count(),
                permits: snapshot.permit_count(),
            };
            (Arc::new(snapshot), None, Some(statistics))
        }
        PolicyInput::NatsKv(policy) => {
            let client = trogon_nats::connect(&config.audit.nats, AUDIT_CONNECT_TIMEOUT)
                .await
                .map_err(StartupError::Audit)?;
            let (snapshot, driver) = KvSnapshot::open(client, policy.clone())
                .await
                .map_err(StartupError::LivePolicy)?;
            let statistics = snapshot.statistics();
            (snapshot, Some(driver), statistics)
        }
    };
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
        agents = statistics.map(|statistics| statistics.agents),
        permits = statistics.map(|statistics| statistics.permits),
        listen = %config.listen,
        trust_domain = %config.trust_domain,
        "egress proxy starting"
    );

    let proxy = Proxy::new(
        snapshot,
        resolver,
        audit,
        config.limits,
        config.non_tls_ports.into_iter().collect(),
    );
    Ok(Server::new(listener, Arc::new(tls), proxy, config.trust_domain).with_policy_driver(policy_driver))
}

fn validate_policy_transport(config: &Config) -> Result<(), StartupError> {
    if let PolicyInput::NatsKv(policy) = &config.policy {
        if !policy.matches_scope(config.audit.identity.tenant(), config.audit.identity.project()) {
            return Err(StartupError::LivePolicyScope);
        }
        KvPolicyConfig::validate_transport(&config.audit.nats).map_err(StartupError::LivePolicyTransport)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
