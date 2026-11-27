use std::path::PathBuf;

use trogon_sandbox_contract::{SubstrateSizeClassError, TokenError};

use crate::actor_index::ActorIndexError;
use crate::adapter::SubstrateError;
use crate::ate_client::ConnectError;
use crate::config::SubstrateConfigError;
use crate::router_tls::RouterTlsError;
use crate::template::TemplateIndexError;

use super::WorkerError;

/// Why the worker process could not serve.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The NATS server could not be reached.
    #[error("failed to connect to NATS at {url}: {source}")]
    Connect {
        url: String,
        #[source]
        source: async_nats::ConnectError,
    },
    /// The size class table is not valid.
    #[error("invalid --size-class {value:?}: {source}")]
    SizeClass {
        value: String,
        #[source]
        source: SubstrateSizeClassError,
    },
    /// Router connections need either complete mutual TLS material or an explicit plaintext opt-in.
    #[error(
        "router connections require complete mTLS material; plaintext local doubles require --allow-insecure-router"
    )]
    RouterTlsIncomplete,
    /// The router TLS material is not usable.
    #[error(transparent)]
    RouterTls(#[from] RouterTlsError),
    /// The assembled configuration is not valid.
    #[error(transparent)]
    Config(#[from] SubstrateConfigError),
    /// The CA certificate for `ate-api-server` could not be read.
    #[error("failed to read TLS CA file at {path}: {source}")]
    TlsCaFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// `ate-api-server` could not be reached.
    #[error("failed to reach ate-api-server at {endpoint}: {source}")]
    Control {
        endpoint: String,
        #[source]
        source: ConnectError,
    },
    /// The template index could not be opened.
    #[error(transparent)]
    TemplateIndex(#[from] TemplateIndexError),
    /// The actor index could not be opened.
    #[error(transparent)]
    ActorIndex(#[from] ActorIndexError),
    /// The provider could not be built.
    #[error(transparent)]
    Provider(#[from] SubstrateError),
    /// The worker identity was rejected.
    #[error(transparent)]
    WorkerId(#[from] TokenError),
    /// The worker loop failed.
    #[error(transparent)]
    Worker(#[from] WorkerError),
}
