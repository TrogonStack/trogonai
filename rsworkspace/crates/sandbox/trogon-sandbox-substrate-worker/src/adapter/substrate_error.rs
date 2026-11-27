use trogon_sandbox_contract::{SandboxIdError, TenantIdError, TokenError};
use trogon_sandbox_protocol::AuthorityResourceIdError;

use crate::agent::AgentError;
use crate::ate_client::AteError;
use crate::config::{SIZE_CLASS_LABEL, SubstrateConfigError};
use crate::naming::NamingError;
use crate::resources::ImageDigestError;
use crate::template::TemplateError;
use crate::workspace::WorkspaceError;

/// Why a substrate provider operation failed.
#[derive(Debug, thiserror::Error)]
pub enum SubstrateError {
    /// A field this provider cannot honor. The broker rejects these before dispatch; this
    /// is the defensive copy, because a worker must not assume the broker validated.
    #[error("{0}")]
    InvalidSpec(String),
    #[error(transparent)]
    Config(#[from] SubstrateConfigError),
    #[error(transparent)]
    ImageDigest(#[from] ImageDigestError),
    #[error(transparent)]
    Tenant(#[from] TenantIdError),
    #[error(transparent)]
    SandboxId(#[from] SandboxIdError),
    #[error(transparent)]
    WorkerId(#[from] TokenError),
    #[error(transparent)]
    Resource(#[from] AuthorityResourceIdError),
    /// The size class table answered with a selector that carries no size label.
    #[error("the substrate size selector carries no {SIZE_CLASS_LABEL} label")]
    UnlabelledSizeSelector,
    #[error(transparent)]
    Naming(#[from] NamingError),
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error(transparent)]
    Ate(#[from] AteError),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("actor ownership storage failed: {0}")]
    Ownership(String),
    #[error("actor {actor} was still {status} after {seconds}s")]
    ResumeTimeout {
        actor: String,
        status: String,
        seconds: u64,
    },
    /// The actor exists but was built from a different template than this lease asked for.
    #[error("actor {actor} runs template {found:?}, not {wanted:?}; a lease name cannot change its image")]
    TemplateMismatch {
        actor: String,
        wanted: String,
        found: String,
    },
}
