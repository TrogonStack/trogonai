//! Command line and environment configuration, validated into typed values.
//!
//! There is deliberately no switch that turns enforcement off or that starts
//! without a policy (ADR#0084): an explicit policy source is required. Audit is required
//! for the same reason (ADR#0085 Decision 5): the tenant, the project, the
//! instance name and the NATS servers have no default, and the NATS connection
//! settings are read the way every NATS client here reads them.

use std::net::SocketAddr;
use std::num::{NonZeroU16, NonZeroU64};
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use trogon_audit::{AuditName, AuditNameError, WriterId, WriterIdentity};
use trogon_nats::{NatsConfig, NatsToken, SubjectTokenViolationError};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::audit_config::AuditConfig;
use crate::connection_limit::{ConnectionLimit, ConnectionLimitError};
use crate::constants::{AUDIT_COMPONENT, MIN_HEAD_BYTES, WRITER_ID_PREFIX};
use crate::limits::Limits;
use crate::live_policy::{KvPolicyConfig, PolicyConfigError, PolicyTransportError};
use crate::mtls::TlsFiles;
use crate::trust_domain::{TrustDomain, TrustDomainError};
use crate::workload_identity_config::{WorkloadIdentityConfig, WorkloadIdentityConfigError};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "trogon-egress-proxy",
    about = "Agent egress proxy: mTLS HTTP CONNECT by exact destination"
)]
pub struct Args {
    /// Address the mTLS listener binds.
    #[arg(long, env = "TROGON_EGRESS_LISTEN", default_value = "127.0.0.1:3128")]
    pub listen: SocketAddr,

    /// Explicit static compatibility policy (TOML).
    #[arg(
        long,
        env = "TROGON_EGRESS_POLICY_FILE",
        required_unless_present = "policy_bucket",
        conflicts_with = "policy_bucket"
    )]
    pub policy_file: Option<PathBuf>,

    /// Existing bounded NATS KV policy bucket. Uses the configured tenant/project key.
    #[arg(
        long,
        env = "TROGON_EGRESS_POLICY_BUCKET",
        requires = "policy_incarnation",
        conflicts_with = "policy_file"
    )]
    pub policy_bucket: Option<String>,

    /// Independently provisioned producer incarnation. Never adopted from the bucket.
    #[arg(long, env = "TROGON_EGRESS_POLICY_INCARNATION", requires = "policy_bucket")]
    pub policy_incarnation: Option<NonZeroU64>,

    /// PEM certificate chain the proxy presents, leaf first.
    #[arg(
        long,
        env = "TROGON_EGRESS_TLS_CERT",
        required_unless_present = "workload_api_endpoint",
        conflicts_with = "workload_api_endpoint"
    )]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key of the proxy certificate.
    #[arg(
        long,
        env = "TROGON_EGRESS_TLS_KEY",
        required_unless_present = "workload_api_endpoint",
        conflicts_with = "workload_api_endpoint"
    )]
    pub tls_key: Option<PathBuf>,

    /// PEM bundle of the trust anchors client certificates must chain to.
    #[arg(
        long,
        env = "TROGON_EGRESS_CLIENT_CA",
        required_unless_present = "workload_api_endpoint",
        conflicts_with = "workload_api_endpoint"
    )]
    pub client_ca: Option<PathBuf>,

    /// Isolated Unix SPIFFE Workload API socket owned by the trusted runtime.
    #[arg(long, env = "TROGON_EGRESS_WORKLOAD_API_ENDPOINT", requires_all = ["workload_spiffe_id", "registration_bucket", "registration_fence", "policy_bucket"])]
    pub workload_api_endpoint: Option<String>,

    /// Exact governed identity this proxy must receive from the Workload API.
    #[arg(long, env = "TROGON_EGRESS_WORKLOAD_SPIFFE_ID", requires = "workload_api_endpoint")]
    pub workload_spiffe_id: Option<String>,

    /// Existing scoped registration projection. Never provisioned by this process.
    #[arg(long, env = "TROGON_EGRESS_REGISTRATION_BUCKET", requires = "workload_api_endpoint")]
    pub registration_bucket: Option<String>,

    /// Independently provisioned recovery fence, never adopted from projection data.
    #[arg(long, env = "TROGON_EGRESS_REGISTRATION_FENCE", requires = "workload_api_endpoint")]
    pub registration_fence: Option<NonZeroU64>,

    /// Trust domain of the agent URI SAN: spiffe://<trust-domain>/agent/<agent-id>.
    #[arg(long, env = "TROGON_EGRESS_TRUST_DOMAIN")]
    pub trust_domain: String,

    /// The one tenant this proxy serves. It is configuration until the
    /// certificate profile carries it.
    #[arg(long, env = "TROGON_EGRESS_TENANT")]
    pub tenant: String,

    /// The one project this proxy serves. It is configuration until the
    /// certificate profile carries it.
    #[arg(long, env = "TROGON_EGRESS_PROJECT")]
    pub project: String,

    /// Name of this proxy instance, distinct per replica. It becomes the audit
    /// writer id, so records say which process wrote them.
    #[arg(long, env = "TROGON_EGRESS_INSTANCE")]
    pub instance: String,

    /// Comma separated NATS server URL(s) of the audit stream. The other
    /// connection settings (credentials, TLS) come from the standard NATS
    /// environment variables.
    #[arg(long, env = "NATS_URL")]
    pub nats_url: String,

    /// Destination ports whose tunnels are not TLS, so the SNI check is
    /// skipped for them. Comma separated; empty by default.
    #[arg(long = "non-tls-port", env = "TROGON_EGRESS_NON_TLS_PORTS", value_delimiter = ',')]
    pub non_tls_ports: Vec<u16>,

    /// Largest accepted CONNECT request head, in bytes.
    #[arg(long, env = "TROGON_EGRESS_MAX_HEAD_BYTES", default_value_t = Limits::default().max_head_bytes)]
    pub max_head_bytes: usize,

    /// Maximum active connections, including handshakes and established tunnels.
    #[arg(long, env = "TROGON_EGRESS_MAX_CONNECTIONS", default_value_t = Limits::default().max_connections.get())]
    pub max_connections: usize,

    /// Seconds a client has to send its first TLS record after the tunnel opens.
    #[arg(long, env = "TROGON_EGRESS_FIRST_RECORD_TIMEOUT_SECS", default_value_t = Limits::default().first_record_timeout.as_secs())]
    pub first_record_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub policy: PolicyInput,
    pub tls: TlsInput,
    pub trust_domain: TrustDomain,
    pub audit: AuditConfig,
    pub non_tls_ports: Vec<NonZeroU16>,
    pub limits: Limits,
}

#[derive(Debug, Clone)]
pub enum PolicyInput {
    StaticFile(PathBuf),
    NatsKv(KvPolicyConfig),
}

#[derive(Debug, Clone)]
pub enum TlsInput {
    Files(TlsFiles),
    Workload(WorkloadIdentityConfig),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
    #[error("select exactly one complete TLS identity source")]
    TlsSource,
    #[error("invalid workload identity configuration")]
    WorkloadIdentity(#[source] WorkloadIdentityConfigError),
    #[error("select exactly one policy source and configure its incarnation")]
    PolicySource,
    #[error("invalid live policy configuration")]
    LivePolicy(#[source] PolicyConfigError),
    #[error("live policy transport is not authenticated")]
    LivePolicyTransport(#[source] PolicyTransportError),
    #[error("invalid trust domain")]
    TrustDomain(#[source] TrustDomainError),
    #[error("invalid tenant")]
    Tenant(#[source] AuditNameError),
    #[error("invalid project")]
    Project(#[source] SubjectTokenViolationError),
    #[error("invalid instance name")]
    Instance(#[source] AuditNameError),
    #[error("the audit component is not a subject token")]
    Component(#[source] SubjectTokenViolationError),
    #[error("--nats-url / NATS_URL must list at least one server")]
    NoNatsServers,
    #[error("a non-TLS port must be between 1 and 65535")]
    ZeroPort,
    #[error("the head limit must be at least {MIN_HEAD_BYTES} bytes")]
    HeadLimitTooSmall,
    #[error("the first record timeout must be at least one second")]
    FirstRecordTimeoutTooSmall,
    #[error("invalid connection limit")]
    ConnectionLimit(#[source] ConnectionLimitError),
}

impl TryFrom<Args> for Config {
    type Error = ConfigError;

    fn try_from(args: Args) -> Result<Self, Self::Error> {
        Self::resolve(args, &SystemEnv)
    }
}

impl Config {
    /// Validates `args`, reading the NATS connection settings from `env`.
    pub fn resolve<E: ReadEnv>(args: Args, env: &E) -> Result<Self, ConfigError> {
        let trust_domain = TrustDomain::new(&args.trust_domain).map_err(ConfigError::TrustDomain)?;
        let audit = audit_config(&args, env)?;
        let policy = match (&args.policy_file, &args.policy_bucket, args.policy_incarnation) {
            (Some(path), None, None) => PolicyInput::StaticFile(path.clone()),
            (None, Some(bucket), Some(incarnation)) => PolicyInput::NatsKv(
                KvPolicyConfig::new(
                    bucket,
                    audit.identity.tenant().clone(),
                    audit.identity.project().clone(),
                    incarnation,
                )
                .map_err(ConfigError::LivePolicy)?,
            ),
            _ => return Err(ConfigError::PolicySource),
        };
        if matches!(policy, PolicyInput::NatsKv(_)) {
            KvPolicyConfig::validate_transport(&audit.nats).map_err(ConfigError::LivePolicyTransport)?;
        }
        let non_tls_ports = args
            .non_tls_ports
            .iter()
            .map(|port| NonZeroU16::new(*port).ok_or(ConfigError::ZeroPort))
            .collect::<Result<Vec<_>, _>>()?;
        if args.max_head_bytes < MIN_HEAD_BYTES {
            return Err(ConfigError::HeadLimitTooSmall);
        }
        if args.first_record_timeout_secs == 0 {
            return Err(ConfigError::FirstRecordTimeoutTooSmall);
        }
        let tls = match (
            &args.tls_cert,
            &args.tls_key,
            &args.client_ca,
            &args.workload_api_endpoint,
            &args.workload_spiffe_id,
            &args.registration_bucket,
            args.registration_fence,
        ) {
            (Some(certificate_chain), Some(private_key), Some(client_trust_anchors), None, None, None, None) => {
                TlsInput::Files(TlsFiles {
                    certificate_chain: certificate_chain.clone(),
                    private_key: private_key.clone(),
                    client_trust_anchors: client_trust_anchors.clone(),
                })
            }
            (None, None, None, Some(endpoint), Some(identity), Some(bucket), Some(fence))
                if matches!(policy, PolicyInput::NatsKv(_)) =>
            {
                TlsInput::Workload(
                    WorkloadIdentityConfig::new(endpoint, identity, bucket, fence, &trust_domain, &audit)
                        .map_err(ConfigError::WorkloadIdentity)?,
                )
            }
            _ => return Err(ConfigError::TlsSource),
        };
        Ok(Self {
            listen: args.listen,
            policy,
            tls,
            trust_domain,
            audit,
            non_tls_ports,
            limits: Limits {
                max_connections: ConnectionLimit::new(args.max_connections).map_err(ConfigError::ConnectionLimit)?,
                max_head_bytes: args.max_head_bytes,
                first_record_timeout: Duration::from_secs(args.first_record_timeout_secs),
                ..Limits::default()
            },
        })
    }
}

fn audit_config<E: ReadEnv>(args: &Args, env: &E) -> Result<AuditConfig, ConfigError> {
    let mut nats = NatsConfig::from_env(env);
    nats.servers = args
        .nats_url
        .split(',')
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .map(str::to_owned)
        .collect();
    if nats.servers.is_empty() {
        return Err(ConfigError::NoNatsServers);
    }
    let tenant = AuditName::new(&args.tenant).map_err(ConfigError::Tenant)?;
    let project = NatsToken::new(&args.project).map_err(ConfigError::Project)?;
    let writer = WriterId::new(format!("{WRITER_ID_PREFIX}{}", args.instance)).map_err(ConfigError::Instance)?;
    let component = NatsToken::new(AUDIT_COMPONENT).map_err(ConfigError::Component)?;
    Ok(AuditConfig {
        nats,
        identity: WriterIdentity::new(writer, component, tenant, project),
    })
}

#[cfg(test)]
mod tests;
