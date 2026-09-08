use std::time::Duration;

use trogon_identity_types::aauth::headers;

/// Defensive recursion bound for `act` chain traversal. Not specified by the
/// draft; chosen generously above any plausible legitimate delegation depth
/// (call chaining hops + at most one sub-agent hop) while still bounding
/// worst-case work for a crafted token.
pub const MAX_CHAIN_DEPTH: usize = 16;

/// Default positive TTL for cached JWK sets. Matches the documented OpenAI
/// Workload Identity Federation discovery cache.
pub const DEFAULT_TTL_SECS: i64 = 600;
/// Default negative TTL for cached failures. Short enough that a transient
/// outage at the upstream IdP self-heals quickly without becoming a tight loop.
pub const DEFAULT_NEGATIVE_TTL_SECS: i64 = 30;
/// Hard cap on the number of distinct issuers held in the cache. `iss` is
/// pulled from the JWT payload before signature verification, so an attacker
/// can drive arbitrary distinct strings into this map. The cap prevents
/// unbounded growth; once reached, a single existing entry is evicted to make
/// room for the new one.
pub const DEFAULT_MAX_ENTRIES: usize = 1024;

/// Maximum number of resolve retries when a concurrent `invalidate*` lands
/// while a fetch is in flight. Bounded so an adversarial invalidate loop
/// cannot pin a caller in an unbounded retry cycle.
pub(crate) const MAX_INVALIDATE_RETRIES: usize = 3;

/// Default cap on a single well-known JWKS response body. JWKS documents are
/// small (a handful of public keys); this is generous headroom while still
/// refusing an issuer that tries to stream gigabytes at the verifier.
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 256 * 1024;
/// Default request timeout for a single well-known fetch attempt.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Floor for the replay-store TTL. The derived TTL is
/// `max(MIN_REPLAY_TTL_SECS, max_skew_secs * 2)` so a zero-skew configuration
/// still keeps nonce entries long enough to refuse the same signed request
/// arriving twice — without this floor a `max_skew_secs = 0` deployment would
/// install nonces with a zero-second TTL and lose replay protection on the
/// next GC pass.
pub(crate) const MIN_REPLAY_TTL_SECS: i64 = 60;

/// Security-sensitive headers that drive PoP verification. If any appears
/// more than once, the verifier refuses rather than silently picking one
/// value and letting the rest go unauthenticated -- mirrors the same
/// defense-in-depth rule [`crate::nats_pop`] applies.
pub(crate) const HTTP_SECURITY_HEADERS: &[&str] = &[
    headers::SIGNATURE_KEY,
    headers::SIGNATURE_INPUT,
    headers::SIGNATURE,
    headers::CONTENT_DIGEST,
    headers::MISSION,
];

/// Security-sensitive headers that drive PoP verification. If any appears more
/// than once (case-insensitive) in a request, the verifier refuses rather
/// than silently picking one value and letting the rest go unauthenticated.
pub(crate) const NATS_SECURITY_HEADERS: &[&str] = &[
    headers::NATS_TOKEN,
    headers::NATS_SIG_INPUT,
    headers::NATS_SIG,
    headers::NATS_SIG_CREATED,
    headers::NATS_SIG_NONCE,
    headers::CONTENT_DIGEST,
    // Not part of the PoP envelope, but verification reads one value while
    // downstream consumers could read another -- the same smuggling shape
    // the six envelope headers are guarded against.
    headers::NATS_AUTH_TOKEN,
    headers::MISSION,
];
