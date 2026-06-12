//! Context Twin derivation and deterministic Prompt Compiler for session projections.

pub mod compiler;
pub mod config;
pub mod context_twin;
pub mod error;
pub mod event;
pub mod input;
pub mod kernel;
pub mod nats;
pub mod store;
pub mod telemetry;
pub mod token_budget;
pub mod update_context;

pub use compiler::{DefaultPromptCompiler, PromptCompiler};
pub use config::ProjectionConfig;
pub use context_twin::derive_context_twin;
pub use error::ProjectionError;
pub use input::ProjectionInput;
pub use kernel::update_context_twin;
pub use store::{ContextTwinStore, provision_context_twin_store};
pub use token_budget::{TokenBudget, estimate_content_tokens, estimate_text_tokens};
pub use update_context::ContextTwinUpdateContext;
