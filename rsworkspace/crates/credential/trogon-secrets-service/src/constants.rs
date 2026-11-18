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
/// every tenant's runtime services reach one queue group. No export gives a
/// tenant account this subject: the caller is a registered PLATFORM trusted
/// caller that names the tenant, and the service authorizes that tenant from
/// its own state (ADR#0095 Decision 5).
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

/// How long the service waits after OpenBao first refuses its AppRole login
/// before it asks again. The wait doubles after each refusal (ADR#0023
/// Decision 3).
pub(crate) const OPENBAO_LOGIN_RETRY_INITIAL: Duration = Duration::from_secs(1);

/// The longest the service waits between two AppRole logins OpenBao refused.
pub(crate) const OPENBAO_LOGIN_RETRY_MAX: Duration = Duration::from_secs(30);

/// How far back an invalidation consumer that resumes reads: the longest a
/// released copy can still be held, the ADR#0049 bound of
/// [`RELEASE_TTL_BOUND`] plus its 30 seconds of per-key jitter. A notice older
/// than this concerns only copies that have already expired.
pub const INVALIDATION_REPLAY_WINDOW: Duration = RELEASE_TTL_BOUND.saturating_add(Duration::from_secs(30));

/// The stream that holds release invalidation notices.
pub const INVALIDATION_STREAM: &str = "SECRETS_INVALIDATION";

/// The subjects of [`INVALIDATION_STREAM`]: every tenant's notices, the tenant
/// account in the fourth token. The literal prefix comes first because the
/// server refuses a stream subject that starts with a wildcard unless it drops
/// publish acks (ADR#0081 Decision 3 amendment).
pub const INVALIDATION_SUBJECTS: &str = "secrets.invalidation.v1.*.>";

/// What precedes the tenant account in a notice subject, which the owner
/// token and the credential id follow.
pub(crate) const INVALIDATION_SUBJECT_INFIX: &str = "secrets.invalidation.v1";

/// How long the invalidation stream keeps a notice: the hour ADR#0083
/// Decision 7 keeps sealing-key invalidations for. It is well past
/// [`INVALIDATION_REPLAY_WINDOW`], and short enough that the stream stays
/// small with no count or byte bound.
pub(crate) const INVALIDATION_MAX_AGE: Duration = Duration::from_secs(3600);

/// How long the invalidation stream remembers a notice's `Nats-Msg-Id`, so a
/// credential event delivered again within it publishes one notice.
pub(crate) const INVALIDATION_DUPLICATE_WINDOW: Duration = Duration::from_secs(120);

/// The largest message the invalidation stream takes, in bytes. The largest
/// notice, a 128 character owner and the longest sequence, fits with its
/// headers several times over, and anything larger is not a notice.
pub(crate) const INVALIDATION_MAX_MESSAGE_SIZE: i32 = 1024;

/// The replicas the invalidation stream must keep when the operator sets no
/// count: the three of a production cluster, which survives losing one node.
/// A single node deployment sets one.
pub(crate) const DEFAULT_INVALIDATION_STREAM_REPLICAS: NonZeroUsize = NonZeroUsize::MIN.saturating_add(2);

/// The durable consumer the invalidation publisher reads the credential stream
/// through. Every member of the queue group shares it, so each credential
/// event is published by one of them, and a member that restarts resumes
/// where the group left off.
pub const INVALIDATION_PUBLISHER_DURABLE: &str = "secrets-invalidation-publisher";

/// How long the credential stream waits for the publisher to settle an event
/// before it delivers the event again. The delivery that follows republishes
/// the same `Nats-Msg-Id`, so it must arrive well inside the invalidation
/// stream's two minute duplicate window to be stored once.
pub(crate) const INVALIDATION_PUBLISHER_ACK_WAIT: Duration = Duration::from_secs(30);

/// How long the credential stream holds back an event the publisher could not
/// publish yet before it delivers the event again. It keeps a retried notice
/// inside the ADR#0049 target of five seconds and far inside the duplicate
/// window, while a stream that refuses every notice is asked once a second
/// per event rather than in a tight loop.
pub(crate) const INVALIDATION_RETRY_DELAY: Duration = Duration::from_secs(1);
