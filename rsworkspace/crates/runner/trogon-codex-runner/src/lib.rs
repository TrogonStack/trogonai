//! trogon-codex-runner — ACP agent backed by OpenAI Codex (`codex app-server`).
//!
//! ## Known limitation: session portability
//!
//! Codex conversation state is provider-owned (`thread_id` in the Codex
//! subprocess). This runner caches session metadata in memory only; there is no
//! NATS KV session store and sessions are **not** portable across runner respawn
//! or cross-runner migration. This is expected behaviour, not a bug.
//!
//! ## Permissions
//!
//! Codex sessions accept Trogon permission modes for ACP compatibility, but the codex
//! `app-server` subprocess executes tools internally. Trogon maps modes to coarse
//! `approvalPolicy` values on `turn/start` and refuses modes it cannot honor (e.g. `plan`).
//! Per-tool permission gating inside the subprocess is not enforced by this runner — see
//! [`permissions`] for details.

pub mod kernel_shadow;
pub mod traits;

mod agent;
mod permissions;
mod process;

pub use agent::{CodexAgent, DefaultCodexAgent, NatsNotifierFactory};
pub use kernel_shadow::{ShadowRecorder, provision as provision_kernel_shadow};
pub use process::{CodexProcess, RealProcessSpawner};
