//! # Permissions
//!
//! Codex sessions accept Trogon permission modes for ACP compatibility, but the codex
//! `app-server` subprocess executes tools internally. Trogon maps modes to coarse
//! `approvalPolicy` values on `turn/start` and refuses modes it cannot honor (e.g. `plan`).
//! Per-tool permission gating inside the subprocess is not enforced by this runner — see
//! [`permissions`] for details.

pub mod traits;

mod agent;
mod permissions;
mod process;

pub use agent::{CodexAgent, DefaultCodexAgent, NatsNotifierFactory};
pub use process::{CodexProcess, RealProcessSpawner};
