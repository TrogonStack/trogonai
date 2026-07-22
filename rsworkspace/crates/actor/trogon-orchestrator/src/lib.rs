#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(coverage, allow(dead_code))] // coverage build cfg-excludes `serve`, orphaning its private server helpers
// Trait impls intentionally return `impl Future + Send` to pin the auto-trait
// bound; converting to `async fn` would drop the explicit `Send` guarantee.
#![allow(clippy::manual_async_fn)]
pub mod caller;
pub mod config;
pub mod engine;
pub mod provider;
pub mod server;
pub mod types;

pub use config::OrchestratorConfig;
pub use engine::OrchestratorEngine;
pub use provider::{AnthropicOrchestratorProvider, OrchestratorAuthStyle, OrchestratorLlmConfig, OrchestratorProvider};
pub use types::{OrchestrationResult, OrchestratorError, SubTask, SubTaskResult, TaskPlan};

pub use server::serve;
