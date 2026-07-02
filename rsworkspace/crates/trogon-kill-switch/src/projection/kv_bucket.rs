use async_nats::jetstream::kv;

/// Bucket name for the kill switch state projection.
///
/// Mirrors `a2a-nats`'s `A2A_AGENT_CARDS` catalog bucket: one JetStream KV
/// bucket, one key per entity id, current-state-only (no history retained
/// beyond the latest revision, since a policy check only ever needs "is this
/// agent killed right now").
pub const KILL_SWITCH_STATE: &str = "KILL_SWITCH_STATE";

/// Kill/revive state values are a few dozen bytes of JSON; this cap is
/// generous headroom, not a sizing exercise, and exists to reject a malformed
/// writer before it can wedge the bucket with an oversized value.
const MAX_VALUE_SIZE: i32 = 4096;

pub fn kill_switch_state_bucket_config() -> kv::Config {
    kv::Config {
        bucket: KILL_SWITCH_STATE.to_owned(),
        history: 1,
        max_value_size: MAX_VALUE_SIZE,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
