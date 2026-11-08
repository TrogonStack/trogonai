//! Command line and environment configuration, validated into typed values.
//!
//! There is deliberately no switch that turns enforcement off or that starts
//! without a policy (ADR#0084): the policy file is required. Audit is required
//! for the same reason (ADR#0085 Decision 5): the tenant, the project, the
//! instance name and the NATS servers have no default, and the NATS connection
//! settings are read the way every NATS client here reads them.

use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use trogon_audit::{AuditName, AuditNameError, WriterId, WriterIdentity};
use trogon_nats::{NatsConfig, NatsToken, SubjectTokenViolationError};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::audit_config::AuditConfig;
use crate::constants::{AUDIT_COMPONENT, MIN_HEAD_BYTES, WRITER_ID_PREFIX};
use crate::limits::Limits;
use crate::mtls::TlsFiles;
use crate::trust_domain::{TrustDomain, TrustDomainError};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "trogon-egress-proxy",
    about = "Agent egress proxy: mTLS HTTP CONNECT by exact destination"
)]
pub struct Args {
    /// Address the mTLS listener binds.
    #[arg(long, env = "TROGON_EGRESS_LISTEN", default_value = "127.0.0.1:3128")]
    pub listen: SocketAddr,

    /// Static policy snapshot (TOML). The proxy refuses to start without it.
    #[arg(long, env = "TROGON_EGRESS_POLICY_FILE")]
    pub policy_file: PathBuf,

    /// PEM certificate chain the proxy presents, leaf first.
    #[arg(long, env = "TROGON_EGRESS_TLS_CERT")]
    pub tls_cert: PathBuf,

    /// PEM private key of the proxy certificate.
    #[arg(long, env = "TROGON_EGRESS_TLS_KEY")]
    pub tls_key: PathBuf,

    /// PEM bundle of the trust anchors client certificates must chain to.
    #[arg(long, env = "TROGON_EGRESS_CLIENT_CA")]
    pub client_ca: PathBuf,

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

    /// Seconds a client has to send its first TLS record after the tunnel opens.
    #[arg(long, env = "TROGON_EGRESS_FIRST_RECORD_TIMEOUT_SECS", default_value_t = Limits::default().first_record_timeout.as_secs())]
    pub first_record_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub policy_file: PathBuf,
    pub tls: TlsFiles,
    pub trust_domain: TrustDomain,
    pub audit: AuditConfig,
    pub non_tls_ports: Vec<NonZeroU16>,
    pub limits: Limits,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
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
        Ok(Self {
            listen: args.listen,
            policy_file: args.policy_file,
            tls: TlsFiles {
                certificate_chain: args.tls_cert,
                private_key: args.tls_key,
                client_trust_anchors: args.client_ca,
            },
            trust_domain,
            audit,
            non_tls_ports,
            limits: Limits {
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
