//! trogon-codex-runner — ACP agent backed by OpenAI Codex (`codex app-server`).
//!
//! ## Known limitation: session portability
//!
//! Codex conversation state is provider-owned (`thread_id` in the Codex
//! subprocess). This runner caches session metadata in memory only; there is no
//! NATS KV session store and sessions are **not** portable across runner respawn
//! or cross-runner migration. This is expected behaviour, not a bug.

pub mod traits;

mod agent;
mod process;

pub use agent::{CodexAgent, DefaultCodexAgent, NatsNotifierFactory};
pub use process::{CodexProcess, RealProcessSpawner};
