pub(super) const BASE_DELAY_MS: f64 = 500.0;

pub(super) const MAX_DELAY_MS: f64 = 30_000.0;

pub(super) const JITTER_MS: f64 = 500.0;

/// The `internal_metadata` keys that only mean something next to a status.
pub(super) const RIDES_A_STATUS: [&str; 2] = ["reply_degraded", "card_turn_stamp"];

/// The riders a `null` outlives the status for.
pub(super) const DELETE_OUTLIVES_STATUS: [&str; 1] = ["card_turn_stamp"];

/// The keys that are re-sent one group at a time after a refusal.
pub(super) const REQUEUED_ALONE: [&str; 1] = ["disabled_mcp_servers"];

pub(super) const COST_KEYS: [&str; 8] = [
    "cumulative_cost_usd",
    "cumulative_input_tokens",
    "cumulative_output_tokens",
    "cumulative_cache_read_tokens",
    "cumulative_cache_write_tokens",
    "cost_state",
    "memory_extract_place",
    "model_turn_stored",
];

pub(super) const OWED_KEYS: [&str; 5] = [
    "last_served_model",
    "worker_permission_mode",
    "plan_doc",
    "card_turn_stamp",
    "card_answer_handed_on",
];
