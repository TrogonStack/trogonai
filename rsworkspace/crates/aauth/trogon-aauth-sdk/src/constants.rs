//! Constants for the `trogon-aauth-sdk` crate.

/// Default poll interval per "Polling with GET": used when a `202` response
/// omits `Retry-After` (the spec calls this a MUST-have header on `202`s, but
/// core stays defensive for malformed servers).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
/// Linear backoff step applied on `429 Too Many Requests`, per "Polling with GET".
pub const SLOW_DOWN_STEP_SECS: u64 = 5;

/// Order of covered components in the `AAuth-Sig-Input` header. Must match
/// `NatsSignatureEnvelope::canonical_base` and what `NatsPopVerifier` expects.
pub(crate) const SIG_INPUT: &str =
    "(\"@subject\" \"@reply\" \"content-digest\" \"aauth-token\" \"aauth-sig-created\" \"aauth-sig-nonce\")";
