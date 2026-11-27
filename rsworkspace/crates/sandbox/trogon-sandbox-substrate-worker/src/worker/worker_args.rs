use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use trogon_sandbox_contract::{SubstrateSizeClasses, WorkerId};
use trogon_sandbox_observability::ObservabilityArgs;

use crate::ate_client::{BearerTokenFile, ConnectSecurity, TlsTrust};
use crate::config::{DEFAULT_ROUTER_ENDPOINT, RouterEndpoint, SandboxClass, SubstrateConfig};
use crate::router_tls::RouterTlsConfig;

use super::ServeError;

/// Command line and environment configuration of the worker.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox substrate worker: serves sandbox lease requests as Agent Substrate actors"
)]
pub struct WorkerArgs {
    #[arg(long, env = "SANDBOX_SUBSTRATE_WORKER_ID")]
    pub worker_id: Option<WorkerId>,

    /// Journal for the tenant and attempt each lease belongs to, so the
    /// dispatch fence survives a restart. Unset keeps it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// `ate-api-server` gRPC address. TLS trust and the caller's identity are configured
    /// separately: see `--tls-ca-file` and `--bearer-token-file`.
    #[arg(long, env = "SANDBOX_SUBSTRATE_API_ENDPOINT")]
    pub ate_api_endpoint: String,

    /// PEM CA certificate that signed `ate-api-server`'s TLS certificate. Substrate's
    /// pod-cert issuer is not a public CA, so this is required for any TLS endpoint. Omit
    /// it to connect over plaintext, such as to a local test double.
    #[arg(long, env = "SANDBOX_SUBSTRATE_TLS_CA_FILE")]
    pub tls_ca_file: Option<PathBuf>,

    /// TLS server name to check `ate-api-server`'s certificate against. Defaults to the
    /// endpoint's own host; only needed when that host would not match the certificate,
    /// such as a port-forward to `localhost`.
    #[arg(long, env = "SANDBOX_SUBSTRATE_TLS_SERVER_NAME")]
    pub tls_server_name: Option<String>,

    /// Bearer token file sent as `authorization: Bearer <token>` on every call. Read fresh
    /// on every call rather than once at startup, so a Kubernetes-rotated projected
    /// service-account token is picked up without a restart. Defaults to the path
    /// Kubernetes projects a pod's own token to.
    #[arg(long, env = "SANDBOX_SUBSTRATE_BEARER_TOKEN_FILE")]
    pub bearer_token_file: Option<PathBuf>,

    /// `host:port` of the substrate router's CONNECT-tunnel listener.
    #[arg(
        long,
        env = "SANDBOX_SUBSTRATE_ROUTER_ENDPOINT",
        default_value = DEFAULT_ROUTER_ENDPOINT
    )]
    pub router_endpoint: RouterEndpoint,

    #[arg(long, env = "SANDBOX_SUBSTRATE_ROUTER_TLS_CA_FILE")]
    pub router_tls_ca_file: Option<PathBuf>,
    #[arg(long, env = "SANDBOX_SUBSTRATE_ROUTER_TLS_CLIENT_CERT_FILE")]
    pub router_tls_client_cert_file: Option<PathBuf>,
    #[arg(long, env = "SANDBOX_SUBSTRATE_ROUTER_TLS_CLIENT_KEY_FILE")]
    pub router_tls_client_key_file: Option<PathBuf>,
    #[arg(long, env = "SANDBOX_SUBSTRATE_ROUTER_TLS_SERVER_NAME")]
    pub router_tls_server_name: Option<String>,
    #[arg(
        long,
        env = "SANDBOX_SUBSTRATE_ALLOW_INSECURE_ROUTER",
        conflicts_with_all = [
            "router_tls_ca_file",
            "router_tls_client_cert_file",
            "router_tls_client_key_file",
            "router_tls_server_name",
        ]
    )]
    pub allow_insecure_router: bool,

    /// Digest-pinned `sandbox-actor-agent` image. Part of the actor template key, so moving
    /// it builds new templates rather than silently reusing the old agent.
    #[arg(long, env = "SANDBOX_SUBSTRATE_AGENT_IMAGE")]
    pub agent_image: String,

    /// Port the agent listens on inside the actor.
    #[arg(long, env = "SANDBOX_SUBSTRATE_AGENT_PORT", default_value_t = 8110)]
    pub agent_port: u16,

    /// `gvisor` or `microvm`; the class used when a lease does not pin an isolation tier.
    #[arg(long, env = "SANDBOX_SUBSTRATE_SANDBOX_CLASS", default_value = "gvisor")]
    pub sandbox_class: SandboxClass,

    #[arg(
        long,
        env = "SANDBOX_SUBSTRATE_ACTOR_CPU_MILLICORES",
        default_value_t = 1000,
        value_parser = clap::value_parser!(u32).range(1..=999_999)
    )]
    pub actor_cpu_millicores: u32,

    /// Base prefix for `snapshotsConfig.location`, e.g. `gs://bucket/snapshots`.
    #[arg(long, env = "SANDBOX_SUBSTRATE_SNAPSHOTS_LOCATION", default_value = "")]
    pub snapshots_location: String,

    /// The worker pools this deployment advertises, as `small=2048,medium=8192,large=32768`.
    /// Memory is a property of the pool an actor lands on, so this table is the only place
    /// `memory_mib` becomes a real allocation.
    #[arg(
        long,
        env = "SANDBOX_SUBSTRATE_SIZE_CLASS",
        default_value = "small=2048,medium=8192,large=32768"
    )]
    pub size_class: String,

    /// Deadline for the golden-snapshot build of a new actor template version.
    #[arg(long, env = "SANDBOX_SUBSTRATE_TEMPLATE_BUILD_TIMEOUT", default_value = "600")]
    pub template_build_timeout_seconds: u64,

    /// Deadline for `ResumeActor` plus the wait for `STATUS_RUNNING`.
    #[arg(long, env = "SANDBOX_SUBSTRATE_RESUME_TIMEOUT", default_value = "120")]
    pub resume_timeout_seconds: u64,

    #[arg(long, env = "SANDBOX_SUBSTRATE_MAX_CONCURRENT_LEASES", default_value_t = 64)]
    pub max_concurrent_leases: u32,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

impl WorkerArgs {
    /// The size class table named by `--size-class`.
    pub fn size_classes(&self) -> Result<SubstrateSizeClasses, ServeError> {
        self.size_class.parse().map_err(|source| ServeError::SizeClass {
            value: self.size_class.clone(),
            source,
        })
    }

    /// The mutual TLS material for router connections.
    ///
    /// Complete material selects TLS. Plaintext needs the explicit opt-in and no material at
    /// all; anything in between is refused rather than silently downgraded.
    pub fn router_tls(&self) -> Result<Option<RouterTlsConfig>, ServeError> {
        match (
            &self.router_tls_ca_file,
            &self.router_tls_client_cert_file,
            &self.router_tls_client_key_file,
        ) {
            (Some(ca), Some(cert), Some(key)) => Ok(Some(RouterTlsConfig::new(
                ca,
                cert,
                key,
                self.router_tls_server_name
                    .clone()
                    .unwrap_or_else(|| self.router_endpoint.host().to_string()),
            )?)),
            (None, None, None) if self.allow_insecure_router => Ok(None),
            _ => Err(ServeError::RouterTlsIncomplete),
        }
    }

    /// The validated provider configuration these arguments describe.
    pub fn substrate_config(&self) -> Result<SubstrateConfig, ServeError> {
        let config = SubstrateConfig {
            ate_api_endpoint: self.ate_api_endpoint.clone(),
            router_endpoint: self.router_endpoint.clone(),
            agent_image: self.agent_image.clone(),
            agent_port: self.agent_port,
            sandbox_class: self.sandbox_class,
            snapshots_location: self.snapshots_location.clone(),
            size_classes: self.size_classes()?,
            template_build_timeout: Duration::from_secs(self.template_build_timeout_seconds),
            resume_timeout: Duration::from_secs(self.resume_timeout_seconds),
            max_concurrent_leases: self.max_concurrent_leases,
            router_tls: self.router_tls()?,
            actor_cpu_millicores: self.actor_cpu_millicores,
        };
        config.validate()?;
        Ok(config)
    }

    /// How the connection to `ate-api-server` is secured and authenticated.
    pub fn connect_security(&self) -> Result<ConnectSecurity, ServeError> {
        let tls = self
            .tls_ca_file
            .as_ref()
            .map(|path| {
                TlsTrust::from_ca_file(path, self.tls_server_name.clone()).map_err(|source| ServeError::TlsCaFile {
                    path: path.clone(),
                    source,
                })
            })
            .transpose()?;
        let bearer_token_file = Some(
            self.bearer_token_file
                .clone()
                .map(BearerTokenFile::new)
                .unwrap_or_else(BearerTokenFile::default_service_account),
        );
        Ok(ConnectSecurity { tls, bearer_token_file })
    }
}
