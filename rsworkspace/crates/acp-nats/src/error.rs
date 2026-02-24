/// JSON-RPC reserved range -32001: agent unavailable / overloaded (retryable).
/// Returned for timeout and request failures; clients should retry with backoff.
pub const AGENT_UNAVAILABLE: i32 = -32001;
