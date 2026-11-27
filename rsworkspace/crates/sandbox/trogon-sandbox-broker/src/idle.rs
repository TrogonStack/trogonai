//! The single idle evaluator shared by every sweeper.
//!
//! Stages are measured from the lease's last activity, so activity restarts
//! the whole chain. Nothing is stored about which stage already fired: a
//! stage that fired changes the lease (Pause sets `Paused`, Release removes
//! it), so the evaluator re-derives "what is due" from the record alone and a
//! stage can never fire twice.
//!
//! A lease paused by the evaluator (`auto_paused_at` set) stays on the chain
//! so the Release stage can still end it at its own time, measured from the
//! last activity (which pausing does not change). The paused budget keeps
//! applying as well: whichever of the two comes first reclaims the lease.
//! A lease paused by request is never touched by the idle chain, because its
//! owner chose to freeze it.
//!
//! A Suspend stage suspends through the regular suspend path. When the
//! provider cannot suspend the lease it degrades to Pause (and is skipped if
//! the provider cannot pause either). A suspended lease leaves the chain: its
//! suspended budget reaps it, and waking it restarts the idle clock.

mod idle_capabilities;
mod idle_chain;
mod idle_pause_backoff;
mod idle_pauser;
mod no_idle_pause;

pub use idle_capabilities::IdleCapabilities;
pub use idle_chain::{
    has_pause_stage, has_suspend_stage, idle_capabilities, idle_stages, last_activity, next_idle_action,
    next_idle_action_with,
};
pub use idle_pause_backoff::IdlePauseBackoff;
pub use idle_pauser::IdlePauser;
pub use no_idle_pause::NoIdlePause;
pub use trogon_sandbox_contract::IdleAction;

#[cfg(test)]
pub(crate) mod tests;
