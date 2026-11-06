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
use trogon_workload_identity::{ActivationAudit, IdentityDriver, IdentityStore};

use crate::audit::AuditSink;
use crate::audit_config::AuditConfig;
use crate::config::{Config, PolicyInput, TlsInput};
use crate::connection::Proxy;
use crate::constants::AUDIT_CONNECT_TIMEOUT;
use crate::live_policy::{KvPolicyConfig, KvSnapshot, LivePolicyError, PolicyTransportError};
use crate::mtls::{TlsError, load_server_config};
use crate::policy_snapshot::{PolicySnapshot, PolicyStatistics};
use crate::resolver::{Resolver, SystemResolver};
use crate::server::Server;
use crate::static_snapshot::{SnapshotError, StaticSnapshot};
use crate::workload_activation_audit::WorkloadActivationAudit;
use crate::workload_registration_driver::{RegistrationDriver, RegistrationSourceError};
use crate::writer_audit::WriterAudit;

enum RuntimeIdentity {
    Fixed(Arc<rustls::ServerConfig>),
    Workload(Box<WorkloadRuntime>),
}

struct WorkloadRuntime {
    identity: IdentityStore,
    driver: IdentityDriver,
    registration: RegistrationDriver,
}

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
    #[error("workload identity requires an activation audit adapter")]
    ActivationAuditRequired,
    #[error("workload identity requires a live exclusive policy source")]
    WorkloadPolicyRequired,
    #[error("workload identity does not match this runtime scope")]
    WorkloadScope,
    #[error("workload registration source is unavailable")]
    Registration(#[source] RegistrationSourceError),
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
    let activation = Arc::new(WorkloadActivationAudit::new(audit.activation_recorder()));
    start_with_sources(config, Arc::new(SystemResolver), Arc::new(audit), Some(activation)).await
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
    start_with_sources(config, resolver, audit, None).await
}

async fn start_with_sources(
    config: Config,
    resolver: Arc<dyn Resolver>,
    audit: Arc<dyn AuditSink>,
    activation: Option<Arc<dyn ActivationAudit>>,
) -> Result<Server, StartupError> {
    validate_policy_transport(&config)?;
    let mut readiness_policy = None;
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
            let policy = if matches!(config.tls, TlsInput::Workload(_)) {
                policy.for_workload_identity()
            } else {
                policy.clone()
            };
            let client = trogon_nats::connect(&config.audit.nats, AUDIT_CONNECT_TIMEOUT)
                .await
                .map_err(StartupError::Audit)?;
            let (snapshot, driver) = KvSnapshot::open(client, policy)
                .await
                .map_err(StartupError::LivePolicy)?;
            readiness_policy = Some(Arc::clone(&snapshot));
            let statistics = snapshot.statistics();
            (snapshot, Some(driver), statistics)
        }
    };
    let tls = match &config.tls {
        TlsInput::Files(files) => RuntimeIdentity::Fixed(Arc::new(
            load_server_config(files, config.trust_domain.clone()).map_err(StartupError::Tls)?,
        )),
        TlsInput::Workload(workload) => {
            let activation = activation.ok_or(StartupError::ActivationAuditRequired)?;
            let (identity, driver) = IdentityStore::new(workload.identity().clone(), activation);
            let client = trogon_nats::connect(&config.audit.nats, AUDIT_CONNECT_TIMEOUT)
                .await
                .map_err(StartupError::Audit)?;
            let registration = RegistrationDriver::open(client, workload.clone(), identity.clone())
                .await
                .map_err(StartupError::Registration)?;
            RuntimeIdentity::Workload(Box::new(WorkloadRuntime {
                identity,
                driver,
                registration,
            }))
        }
    };
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
    let server = match tls {
        RuntimeIdentity::Fixed(tls) => Server::new(listener, tls, proxy, config.trust_domain),
        RuntimeIdentity::Workload(workload) => {
            let WorkloadRuntime {
                identity,
                driver,
                registration,
            } = *workload;
            let readiness = crate::Readiness::new(Some(identity.clone()), readiness_policy, proxy.audit());
            Server::new_workload(listener, identity, proxy, config.trust_domain, driver, registration)
                .with_readiness(readiness)
        }
    };
    Ok(server.with_policy_driver(policy_driver))
}

fn validate_policy_transport(config: &Config) -> Result<(), StartupError> {
    if let TlsInput::Workload(workload) = &config.tls {
        if !matches!(config.policy, PolicyInput::NatsKv(_)) {
            return Err(StartupError::WorkloadPolicyRequired);
        }
        let local = workload.identity().local();
        if local.tenant() != config.audit.identity.tenant().as_str()
            || local.project() != config.audit.identity.project().as_str()
            || local.spiffe_id().trust_domain().to_string() != config.trust_domain.as_str()
        {
            return Err(StartupError::WorkloadScope);
        }
    }
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
