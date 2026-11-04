//! Command line and environment configuration, validated into typed values.
//!
//! There is deliberately no switch that turns enforcement off or that starts
//! without a policy (ADR#0084): the policy file is required.

use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::constants::MIN_HEAD_BYTES;
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
    pub non_tls_ports: Vec<NonZeroU16>,
    pub limits: Limits,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid trust domain")]
    TrustDomain(#[source] TrustDomainError),
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
        let trust_domain = TrustDomain::new(&args.trust_domain).map_err(ConfigError::TrustDomain)?;
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
            non_tls_ports,
            limits: Limits {
                max_head_bytes: args.max_head_bytes,
                first_record_timeout: Duration::from_secs(args.first_record_timeout_secs),
                ..Limits::default()
            },
        })
    }
}

#[cfg(test)]
mod tests;
