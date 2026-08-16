//! Wire and configuration constants for the ADR#0057 binding.

use std::time::Duration;

/// How long the host waits for its first NATS connection.
pub const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Subject namespace a host answers under when none is configured.
pub const DEFAULT_SUBJECT_PREFIX: &str = "decider";

/// Queue group replicas share so a command reaches exactly one of them.
pub const DEFAULT_QUEUE_GROUP: &str = "q";

/// JetStream stream holding decider events when none is configured.
pub const DEFAULT_EVENTS_STREAM: &str = "DECIDER_EVENTS";

/// Key/Value bucket holding decider snapshots when none is configured.
pub const DEFAULT_SNAPSHOT_BUCKET: &str = "DECIDER_SNAPSHOTS";

/// Object store bucket holding decider components when none is configured.
pub const DEFAULT_MODULE_BUCKET: &str = "DECIDER_MODULES";

/// Module store scheme naming a JetStream object store bucket.
pub const MODULE_STORE_OBJECT_SCHEME: &str = "objectstore";

/// Module store scheme naming a local directory.
pub const MODULE_STORE_DIRECTORY_SCHEME: &str = "file";

/// Concurrent executions allowed when no admission limit is configured.
///
/// Bounded rather than unbounded by default: the limit exists to cap how many
/// guest linear memories can be alive at once, and a host that only gains that
/// cap once someone configures it does not have it on the day it matters.
pub const DEFAULT_ADMISSION_LIMIT: usize = 32;

/// How long JetStream remembers an event id when none is configured.
///
/// The idempotency guarantee a `Trogon-Command-Id` buys is only as long as this
/// window: past it, JetStream has forgotten the derived event ids and a retry
/// appends a second copy. Two minutes matches the NATS server's own default, so
/// an unconfigured host behaves the way its operators already expect.
pub const DEFAULT_DUPLICATE_WINDOW_SECS: u64 = 120;

/// Segment between a module's name and a stream id in an event subject.
pub const EVENT_SUBJECT_SEGMENT: &str = "events";

/// Prefix every `CommandType` carries, per the canonical `Any` type URL form.
pub const TYPE_URL_PREFIX: &str = "type.googleapis.com/";

/// Caller-supplied identity for one command, reused across its retries.
pub const TROGON_COMMAND_ID_HEADER: &str = "Trogon-Command-Id";

/// Stream revision the caller believes it is acting on.
pub const TROGON_EXPECTED_REVISION_HEADER: &str = "Trogon-Expected-Revision";

/// Reply-side discriminant mirroring the `CommandOutcome` arm in the body.
pub const TROGON_DECIDER_OUTCOME_HEADER: &str = "Trogon-Decider-Outcome";

/// Payload encoding header. Protobuf is the only encoding this binding accepts.
pub const CONTENT_TYPE_HEADER: &str = "Content-Type";

/// The only `Content-Type` a decider command may carry.
pub const PROTOBUF_CONTENT_TYPE: &str = "application/protobuf";

/// Environment variable naming the subject namespace.
pub const ENV_DECIDER_SUBJECT_PREFIX: &str = "DECIDER_SUBJECT_PREFIX";

/// Environment variable naming the queue group.
pub const ENV_DECIDER_QUEUE_GROUP: &str = "DECIDER_QUEUE_GROUP";

/// Environment variable naming the JetStream stream that holds decider events.
pub const ENV_DECIDER_EVENTS_STREAM: &str = "DECIDER_EVENTS_STREAM";

/// Environment variable naming the Key/Value bucket that holds snapshots.
pub const ENV_DECIDER_SNAPSHOT_BUCKET: &str = "DECIDER_SNAPSHOT_BUCKET";

/// Environment variable carrying a comma-separated list of `{name}@{version}`
/// module references.
pub const ENV_DECIDER_MODULES: &str = "DECIDER_MODULES";

/// Environment variable naming the store those references resolve against.
pub const ENV_DECIDER_MODULE_STORE: &str = "DECIDER_MODULE_STORE";

/// Environment variable carrying the admission limit, per ADR#0028.
pub const ENV_DECIDER_ADMISSION_LIMIT: &str = "DECIDER_ADMISSION_LIMIT";

/// Environment variable carrying the per-command replay ceiling.
pub const ENV_DECIDER_REPLAY_LIMIT: &str = "DECIDER_REPLAY_LIMIT";

/// Environment variable carrying the JetStream duplicate window, in seconds.
pub const ENV_DECIDER_DUPLICATE_WINDOW_SECS: &str = "DECIDER_DUPLICATE_WINDOW_SECS";

/// Separator between a module's name and version, as an operator writes it.
pub const MODULE_REFERENCE_SEPARATOR: char = '@';

/// Separator the object-store key uses instead, since object names admit `/`
/// and reject `@`.
pub const MODULE_OBJECT_KEY_SEPARATOR: char = '/';
