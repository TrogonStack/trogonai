//! NATS host for WASM decider modules.
//!
//! Implements the [ADR#0057](../../../../docs/adr/0057-decider-command-nats-binding.md)
//! binding: one core request/reply subscription over a subject subtree, each
//! subject naming the protobuf command type it carries, every reply a single
//! `CommandOutcome`.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod command_subject;
pub mod config;
pub mod constants;
pub mod event_subject;
pub mod module_reference;
pub mod module_source;
pub mod outcome;
pub mod request;
pub mod runtime;

pub use command_subject::{CommandSubjectError, CommandSubjects, SubjectPrefix};
pub use config::{Args, ConfigError, ModuleStore, ServerConfig, base_config};
pub use event_subject::{EventSubjectError, ModuleEventSubjects};
pub use module_reference::{ModuleReference, ModuleReferenceError};
pub use module_source::{
    FileModuleSource, FileModuleSourceError, ModuleSource, ObjectStoreModuleSource, ObjectStoreModuleSourceError,
    OpenModuleBucketError, ProvisionModuleBucketError, PublishModuleError,
};
pub use outcome::CommandReply;
pub use request::{CommandRequest, CommandRequestError};
pub use runtime::{CommandRouter, DeciderHost, RoutedCommand, ServeError, StartupError, serve, serve_until};
