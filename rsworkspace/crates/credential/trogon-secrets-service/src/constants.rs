use std::num::NonZeroUsize;
use std::time::Duration;

/// The longest a released value may be held, per the ADR#0049 staleness bound.
///
/// A policy may shorten it and never lengthen it, so a revoked credential
/// leaves every holder within this window.
pub const RELEASE_TTL_BOUND: Duration = Duration::from_secs(300);

/// The generation a single released value reports.
///
/// Only a bundle of several values has generations to commit; a release of one
/// value has none, which the wire contract spells as zero.
pub const UNBUNDLED_GENERATION: u64 = 0;

/// Why a credential state read that found its bound gone answers nothing.
pub(crate) const BOUND_VANISHED: &str = "the last event of the credential's subject was removed during the read";

/// The destination a release record names when `host:port` is longer than an
/// audit name may be (a 250 byte host and a port do not fit). It is the egress
/// proxy's spelling, so both records of the dual audit (ADR#0023 Decision 6)
/// name such a destination alike. The release is still recorded; only the
/// spelling of where is replaced.
pub(crate) const DESTINATION_TOO_LONG_NAME: &str = "destination_too_long";

/// How long the service works on one release before it answers
/// `UNAVAILABLE`. It is shorter than the caller's two second default, so a
/// slow store or stream reaches the caller as a typed failure it backs off
/// from rather than as its own timeout.
pub const RELEASE_DEADLINE: Duration = Duration::from_millis(1500);

/// The subscription the release endpoint answers on: the literal principal
/// kind `service`, with the tenant account and the service id as wildcards, so
/// every tenant's runtime services reach one queue group. The PLATFORM export
/// of `*.secrets.v1.>` pins the first token to the importing account, so the
/// tenant a subject names is the caller's own (ADR#0082 Decision 2).
pub const RELEASE_SUBSCRIPTION: &str = "*.secrets.v1.service.*.SecretExchangeService.ResolveForRelease";

/// The delay a `THROTTLED` answer asks its caller to wait. Every release in
/// flight has answered by then, so the retry finds room unless newer calls
/// took it first.
pub const THROTTLED_RETRY_AFTER: Duration = RELEASE_DEADLINE;

/// How many releases one member of the queue group works on at once when the
/// operator sets no limit.
pub(crate) const DEFAULT_ADMISSION_LIMIT: NonZeroUsize = NonZeroUsize::MIN.saturating_add(63);

/// The characters the contract's `egress_host.host_is_exact` rule keeps out
/// of a host, so a destination is never a pattern or a path.
pub(crate) const NOT_EXACT_HOST_CHARACTERS: [char; 3] = ['*', '/', ' '];

/// The length of a NATS curve public key: the `X` prefix and 55 base32
/// characters.
pub(crate) const CURVE_KEY_LEN: usize = 56;

/// The component the service's audit records name as their writer's.
pub(crate) const AUDIT_COMPONENT: &str = "secrets_service";

/// What the audit writer id of one member of the queue group starts with; the
/// instance the operator names follows it.
pub(crate) const AUDIT_WRITER_PREFIX: &str = "secrets-service:";

/// How long one attempt to reach a NATS server may take. The connection keeps
/// dialing in the background after it, and the service refuses to start if
/// the credential stream cannot be read over it.
pub(crate) const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
