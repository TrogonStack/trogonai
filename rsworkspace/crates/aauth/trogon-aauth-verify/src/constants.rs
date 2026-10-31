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

/// NATS KV bucket every HTTP edge instance shares for replay records, so a
/// signed request admitted by one instance is refused by every other.
pub const AAUTH_REPLAY_BUCKET: &str = "AAUTH_REPLAY";

/// One revision per key: a replay record is written once and never updated,
/// so any older revision would only be dead weight.
pub const AAUTH_REPLAY_HISTORY: i64 = 1;

/// Upper bound on the span between a signature's `created` and `expires`
/// (ADR#0051 Decision 2).
pub const MAX_SIGNATURE_VALIDITY_SECS: u32 = 120;

/// Signature validity a verifier assumes when none is configured.
pub const DEFAULT_SIGNATURE_VALIDITY_SECS: u32 = 60;

/// Upper bound on the clock skew, and on the expiry leeway, a verifier
/// tolerates (ADR#0051 Decision 2).
pub const MAX_CLOCK_SKEW_SECS: u32 = 30;

/// Clock skew a verifier tolerates when none is configured.
pub const DEFAULT_CLOCK_SKEW_SECS: u32 = 30;

/// Lifetime of a replay record and maximum age of [`AAUTH_REPLAY_BUCKET`]:
/// skew is bidirectional, so a signature admitted at `created - skew` stays
/// admissible until `expires + skew`, and its record has to outlive that
/// whole span (ADR#0051 Decision 4).
pub const REPLAY_RECORD_LIFETIME_SECS: u32 = MAX_SIGNATURE_VALIDITY_SECS + 2 * MAX_CLOCK_SKEW_SECS;

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
