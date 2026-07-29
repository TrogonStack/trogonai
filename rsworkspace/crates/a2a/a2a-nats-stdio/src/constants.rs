//! Crate-wide constants for a2a-nats-stdio.

pub(crate) const ENV_A2A_AGENT_ID: &str = "A2A_AGENT_ID";

pub(crate) const METHOD_NOT_FOUND: i32 = -32601;
pub(crate) const INVALID_PARAMS: i32 = -32602;

pub(crate) const CHANNEL_CAP: usize = 128;
/// Cap concurrent in-flight dispatch tasks. A fast producer on stdin can
/// otherwise create unbounded RPC/network work and memory pressure.
pub(crate) const MAX_INFLIGHT_DISPATCH: usize = 64;
