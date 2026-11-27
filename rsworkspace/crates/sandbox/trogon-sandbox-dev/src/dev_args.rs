use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::Parser;
use secrecy::{ExposeSecret, SecretString};
use trogon_sandbox_auth::{BearerToken, BearerTokenError};
use trogon_sandbox_contract::TenantId;
use trogon_sandbox_protocol::{DispatchSigner, ENV_PUBLIC_KEYS, ENV_SIGNING_KEY, ENV_VERIFY_MODE};

use crate::{DevPlan, PlanError, ProcessSpec, WorkerKind};

/// Command-line flags of the local stack. Flag values are parsed into their domain types by clap.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "trogon-sandbox-dev",
    version,
    about = "Start nats-server, the broker, a worker and the ws gateway in one command"
)]
pub struct DevArgs {
    /// Client port of the nats-server this command starts.
    #[arg(long, env = "SANDBOX_DEV_NATS_PORT", default_value_t = 4222)]
    pub nats_port: u16,

    /// Address the ws gateway listens on.
    #[arg(long, env = "SANDBOX_DEV_GATEWAY_LISTEN", default_value = "127.0.0.1:8443")]
    pub gateway_listen: SocketAddr,

    #[arg(long, value_enum, env = "SANDBOX_DEV_WORKER", default_value = "auto")]
    pub worker: WorkerKind,

    /// Image the docker worker uses when a lease does not name one.
    #[arg(long, env = "SANDBOX_IMAGE", default_value = "alpine:3.20")]
    pub image: String,

    /// In-sandbox agent binary; required by the subprocess worker.
    #[arg(long, env = "SANDBOX_AGENT_BINARY")]
    pub agent_binary: Option<PathBuf>,

    /// Isolation backend of the subprocess worker (for example `none`).
    #[arg(long, env = "SANDBOX_ISOLATION_BACKEND")]
    pub isolation_backend: Option<String>,

    /// Directory holding the sandbox binaries. Searched before the directory
    /// of this executable and PATH.
    #[arg(long, env = "SANDBOX_DEV_BIN_DIR")]
    pub bin_dir: Option<PathBuf>,

    /// API key the gateway accepts. A random one is minted when omitted.
    #[arg(long, env = "SANDBOX_DEV_API_KEY", hide_env_values = true)]
    pub api_key: Option<SecretString>,

    /// Write the API key to a new file; defaults to a private temporary file.
    #[arg(long, env = "SANDBOX_DEV_API_KEY_FILE")]
    pub api_key_file: Option<PathBuf>,

    /// Tenant the dev API key belongs to.
    #[arg(long, default_value = "dev")]
    pub tenant: TenantId,

    /// Keep JetStream data in this directory instead of a throwaway one.
    #[arg(long)]
    pub store_dir: Option<PathBuf>,
}

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

impl DevArgs {
    pub fn nats_url(&self) -> String {
        format!("nats://127.0.0.1:{}", self.nats_port)
    }

    /// Parses the supplied API key, if any, into a [`BearerToken`].
    pub fn bearer_token(&self) -> Result<Option<BearerToken>, BearerTokenError> {
        self.api_key
            .as_ref()
            .map(|key| BearerToken::new(key.expose_secret()))
            .transpose()
    }

    /// Builds the process plan. `locate` resolves a binary name to a path so
    /// tests can inject fakes.
    pub fn plan(
        &self,
        worker: WorkerKind,
        store_dir: &Path,
        workspace_root: &Path,
        locate: impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<DevPlan, PlanError> {
        let find = |name: &'static str| locate(name).ok_or(PlanError::BinaryNotFound { name });
        let nats_url = self.nats_url();

        let nats = ProcessSpec {
            name: "nats-server",
            program: locate("nats-server").ok_or(PlanError::NatsServerNotFound)?,
            args: vec![
                "-js".into(),
                "-a".into(),
                "127.0.0.1".into(),
                "-p".into(),
                self.nats_port.to_string(),
                "-sd".into(),
                store_dir.display().to_string(),
            ],
            env: BTreeMap::new(),
        };

        // A throwaway key per run: the broker signs, the worker requires it.
        let (signing_key, public_key) = DispatchSigner::generate_keypair_b64();
        let broker = ProcessSpec {
            name: "trogon-sandbox-broker",
            program: find("trogon-sandbox-broker")?,
            args: vec![],
            env: env_of(&[
                ("NATS_URL", &nats_url),
                ("SANDBOX_QUOTA_BACKEND", "in-memory"),
                ("SANDBOX_IDEMPOTENCY_BACKEND", "in-memory"),
                ("SANDBOX_JETSTREAM_REPLICAS", "1"),
                ("SANDBOX_INSECURE_DEV", "true"),
                (ENV_SIGNING_KEY, &signing_key),
            ]),
        };

        let mut worker_env = env_of(&[
            ("SANDBOX_NATS_URL", &nats_url),
            (ENV_VERIFY_MODE, "require"),
            (ENV_PUBLIC_KEYS, &format!("k1:{public_key}")),
        ]);
        match worker {
            WorkerKind::Docker => {
                worker_env.insert("SANDBOX_IMAGE".into(), self.image.clone());
                worker_env.insert(
                    "SANDBOX_DOCKER_STATE_DIR".into(),
                    store_dir.join("docker-worker").display().to_string(),
                );
            }
            WorkerKind::Subprocess => {
                let agent = self.agent_binary.as_ref().ok_or(PlanError::AgentBinaryRequired)?;
                worker_env.insert("SANDBOX_AGENT_BINARY".into(), agent.display().to_string());
                worker_env.insert("SANDBOX_WORKSPACE_ROOT".into(), workspace_root.display().to_string());
                if let Some(backend) = &self.isolation_backend {
                    worker_env.insert("SANDBOX_ISOLATION_BACKEND".into(), backend.clone());
                }
            }
            WorkerKind::Memory | WorkerKind::Auto => {}
        }
        let worker_name = worker.binary_name();
        let worker_spec = ProcessSpec {
            name: "worker",
            program: find(worker_name)?,
            args: vec![],
            env: worker_env,
        };

        let gateway = ProcessSpec {
            name: "trogon-sandbox-ws-gateway",
            program: find("trogon-sandbox-ws-gateway")?,
            args: vec![
                "--listen".into(),
                self.gateway_listen.to_string(),
                "--nats-url".into(),
                nats_url.clone(),
            ],
            env: BTreeMap::new(),
        };

        Ok(DevPlan {
            gateway_http_url: format!("http://{}", self.gateway_listen),
            gateway_ws_url: format!("ws://{}", self.gateway_listen),
            nats_url,
            nats,
            services: vec![broker, worker_spec, gateway],
            worker,
        })
    }
}
