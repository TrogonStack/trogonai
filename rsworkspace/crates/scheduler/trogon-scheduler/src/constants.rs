//! Crate-wide constants.

use std::num::NonZeroU64;

use trogon_decider_runtime::FrequencySnapshot;

// kv: shared NATS plumbing (event stream and command snapshot bucket).
pub const EVENTS_STREAM: &str = "SCHEDULER_EVENTS";
pub const EVENTS_SUBJECT_PREFIX: &str = "scheduler.schedules.events.";
pub const EVENTS_SUBJECT_PATTERN: &str = "scheduler.schedules.events.>";
pub const COMMAND_SNAPSHOT_BUCKET: &str = "scheduler_command_snapshots";

// reconciliation::request: scheduler-owned NATS header names.
pub(crate) const NATS_SCHEDULE_HEADER: &str = "Nats-Schedule";
pub(crate) const NATS_SCHEDULE_TIME_ZONE_HEADER: &str = "Nats-Schedule-Time-Zone";
pub(crate) const NATS_SCHEDULE_TARGET_HEADER: &str = "Nats-Schedule-Target";
pub(crate) const NATS_SCHEDULE_TTL_HEADER: &str = "Nats-Schedule-TTL";
pub(crate) const NATS_SCHEDULE_SOURCE_HEADER: &str = "Nats-Schedule-Source";
pub(crate) const CONTENT_TYPE_HEADER: &str = "Content-Type";
pub(crate) const TROGON_SCHEDULE_ID_HEADER: &str = "Trogon-Schedule-Id";
pub(crate) const TROGON_SCHEDULE_OCCURRENCE_SEQUENCE_HEADER: &str = "Trogon-Schedule-Occurrence-Sequence";
pub(crate) const TROGON_SCHEDULE_OCCURRENCE_AT_HEADER: &str = "Trogon-Schedule-Occurrence-At";
pub(crate) const TROGON_SCHEDULE_RESERVED_PREFIX: &str = "Trogon-Schedule";

pub(crate) const NATS_RESERVED_HEADERS: [&str; 6] = [
    "Nats-Msg-Id",
    "Nats-Schedule",
    "Nats-Schedule-Source",
    "Nats-Schedule-Target",
    "Nats-Schedule-Time-Zone",
    "Nats-Schedule-TTL",
];

// reconciliation::reconcile
pub(crate) const CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE: &str = "trogon.scheduler.corrupt-checkpoint";

/// One-shot schedules past-due by no more than this window still publish: the
/// execution stream fires an `@at` in the past immediately, so a schedule that
/// is late only because of processing lag is delivered instead of silently
/// expired. Anything older (e.g. replayed history) expires without publishing.
pub(crate) const PAST_AT_GRACE: chrono::Duration = chrono::Duration::minutes(5);

// reconciliation::wakeup
pub const RRULE_WAKEUP_FILTER: &str = "scheduler.schedules.execution.v1.rrule.>";
pub const RRULE_WAKEUP_CONSUMER: &str = "scheduler_rrule_wakeup_v1";

// reconciliation::schedule_subject
pub(crate) const EXECUTION_SUBJECT_PREFIX: &str = "scheduler.schedules.execution.v1";
pub(crate) const EVENT_SUBJECT_PREFIX: &str = "scheduler.schedules.events.v1";
pub(crate) const RRULE_WAKEUP_SUBJECT_PREFIX: &str = "scheduler.schedules.execution.v1.rrule";
/// Namespace reserved for scheduler-internal sentinel routes (e.g. the
/// corrupt-checkpoint placeholder). Reserving it at request validation keeps
/// sentinel routes unclaimable by user schedules, so sentinel detection can
/// never misclassify a real schedule.
pub(crate) const SCHEDULER_INTERNAL_PREFIX: &str = "trogon.scheduler";

// reconciliation::go_duration
pub(crate) const GO_DURATION_MAX_NANOS: u128 = i64::MAX as u128;

// execution::checkpoints::store
pub(crate) const CHECKPOINT_KEY_PREFIX: &str = "v1.";

// execution::worker::consumer
/// Event stream the scheduler consumes persisted schedule events from.
pub const SCHEDULE_EVENT_STREAM: &str = "SCHEDULER_SCHEDULE_EVENTS";
/// Subject the scheduler consumer filters on. Must stay
/// `{EVENT_SUBJECT_PREFIX}.>`; a test enforces the
/// derivation since consts cannot be concatenated at compile time.
pub const SCHEDULE_EVENT_FILTER: &str = "scheduler.schedules.events.v1.>";
/// Durable name the scheduler pull consumer registers under.
pub const SCHEDULE_EVENT_CONSUMER: &str = "scheduler_execution_v1";
/// KV bucket holding the rebuildable scheduler checkpoint cache.
pub const SCHEDULE_STATE_BUCKET: &str = "SCHEDULER_SCHEDULE_STATE";
/// Execution schedule stream (must be provisioned with `AllowMsgSchedules`).
pub const SCHEDULE_EXECUTION_STREAM: &str = "SCHEDULER_SCHEDULE_EXECUTION";

// execution::worker::dispatcher
/// Deliveries after which a missing-checkpoint record stops retrying and is
/// poisoned with a durable failure record. With the consumer's `max_deliver: -1`
/// and the 30s nak-delay cap this bounds the retry window to roughly an hour
/// instead of occupying an ack-pending slot forever.
pub(crate) const MISSING_CHECKPOINT_DELIVERY_CEILING: i64 = 120;

// execution::execution_schedules
pub(crate) const NATS_MSG_ID_HEADER: &str = "Nats-Msg-Id";

// projections::schedules::storage
/// KV bucket holding the schedules read model.
pub const SCHEDULES_BUCKET: &str = "scheduler_schedules";

/// Key of the catch-up checkpoint entry within [`SCHEDULES_BUCKET`].
///
pub const SCHEDULES_CHECKPOINT_KEY: &str = "_query.schedules.read_model.v3.last_event_sequence";

// projections::postgres::store
/// This projection's id in the shared `jetstream_projection_checkpoint` table.
#[cfg(all(feature = "postgres", not(coverage)))]
pub(crate) const SCHEDULES_CHECKPOINT_ID: &str = "schedules_read_model";

#[cfg(all(feature = "postgres", not(coverage)))]
macro_rules! select_columns {
    () => {
        "SELECT schedule_id, status, completed, next_occurrence_at, last_occurrence_at, \
         schedule_kind, at_at, every_seconds, cron_expr, rrule, rrule_dtstart, timezone, rrule_rdate, rrule_exdate, \
         delivery_kind, delivery_subject, delivery_ttl_seconds, delivery_source_subject, \
         message_content_type, message_body, message_headers \
         FROM schedules_projection"
    };
}

#[cfg(all(feature = "postgres", not(coverage)))]
pub(crate) const SELECT_PROJECTION_BY_ID: &str = concat!(select_columns!(), " WHERE schedule_id = $1");
#[cfg(all(feature = "postgres", not(coverage)))]
pub(crate) const SELECT_ALL_PROJECTIONS: &str = concat!(select_columns!(), " ORDER BY schedule_id");

// commands::schedule_next_occurrence
/// Occurrences whose due instant is at most this far in the past are still armed,
/// absorbing scheduling/processing latency around recently due occurrences.
pub(crate) const PAST_OCCURRENCE_GRACE: chrono::Duration = chrono::Duration::minutes(5);

// commands::snapshot
pub(crate) const COMMAND_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).unwrap();

pub(crate) const COMMAND_SNAPSHOT_POLICY: FrequencySnapshot = FrequencySnapshot::new(COMMAND_SNAPSHOT_EVERY);

// telemetry::metrics
pub(crate) const METER_NAME: &str = "trogon-scheduler";
