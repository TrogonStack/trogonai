//! NATS host for WASM decider modules.
//!
//! Implements the [ADR#0057](../../../../../docs/adr/0057-decider-command-nats-binding.md)
//! binding: one core request/reply subscription on the `Decide` endpoint of
//! `trogonai.decider.v1.DeciderService`, each request naming the protobuf
//! command type it carries, and success and failure discriminated by the
//! `Nats-Service-Error-Code` header per ADR#0016.
//!
//! Per [ADR#0082](../../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md),
//! the endpoint subject carries the caller's principal, the host reads it from
//! the delivered subject before it decodes anything, and every execution goes
//! through an authorizer.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod authorizer;
pub mod config;
pub mod constants;
pub mod endpoint;
pub mod event_subject;
pub mod module_reference;
pub mod module_source;
pub mod outcome;
pub mod principal_prefix;
pub mod request;
pub mod runtime;
pub mod status;
mod subject_principal;
pub mod tenant_position;

pub use authorizer::AdmitAnyPrincipal;
pub use config::{Args, ConfigError, ModuleStore, ServerConfig, base_config};
pub use endpoint::{CommandEndpoint, CommandEndpointError, CommandSubjectError, SubjectPrefix};
pub use event_subject::{EventSubjectError, ModuleEventSubjects};
pub use module_reference::{ModuleReference, ModuleReferenceError};
pub use module_source::{
    FileModuleSource, FileModuleSourceError, ModuleSource, ObjectStoreModuleSource, ObjectStoreModuleSourceError,
    OpenModuleBucketError, ProvisionModuleBucketError, PublishModuleError,
};
pub use outcome::CommandReply;
pub use principal_prefix::{PrincipalPrefix, PrincipalPrefixError};
pub use request::{CommandRequest, CommandRequestError};
pub use runtime::{CommandRouter, DeciderHost, RoutedCommand, ServeError, StartupError, serve, serve_until};
pub use status::{FaultClass, find_detail};
pub use tenant_position::{TenantPosition, TenantPositionError};
