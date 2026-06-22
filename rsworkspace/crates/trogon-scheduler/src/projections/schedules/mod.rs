use std::collections::{BTreeMap, HashSet};

use async_nats::jetstream::{
    self,
    consumer::{DeliverPolicy, ReplayPolicy, pull},
    kv,
};
use buffa::MessageField;
use futures::StreamExt;
use trogon_decider_nats::record_stream_message;
use trogon_decider_runtime::{Event, EventData, EventDecode, StreamEvent, StreamPosition};
use trogon_nats::jetstream::{JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamGetStream};

use crate::{
    ScheduleEventCase,
    error::SchedulerError,
    kv::{EVENTS_SUBJECT_PATTERN, EVENTS_SUBJECT_PREFIX, open_events_stream},
    projections_v1, v1,
};

use storage::{SCHEDULES_CHECKPOINT_KEY, get_or_create_schedules_bucket, read_model_key};

/// The read model's KV storage contract (bucket, key scheme, checkpoint key),
/// owned by the projection that defines that layout.
pub(crate) mod storage;

// The schedules read-model projection: it folds the schedule event stream
// (`v1` event protos) directly into the stored KV view (`projections_v1::ScheduleProjection`
// protos). It deals only in protobuf — event proto in, KV proto out — and has no
// dependency on the read-model value objects or the query side. Decoding the
// stored proto back into the read model that callers see lives in `crate::queries`.

/// A change to apply to the KV bucket for a single schedule.
#[derive(Clone, PartialEq)]
enum ProjectionChange {
    Upsert(projections_v1::ScheduleProjection),
    Delete(String),
}

/// The folded state of one schedule stream.
#[derive(Clone, PartialEq)]
enum ScheduleStreamState {
    Initial,
    Present(projections_v1::ScheduleProjection),
    Deleted(String),
}

impl std::fmt::Debug for ScheduleStreamState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initial => write!(f, "Initial"),
            Self::Present(view) => write!(f, "Present({})", view.schedule_id),
            Self::Deleted(id) => write!(f, "Deleted({id})"),
        }
    }
}

impl std::fmt::Debug for ProjectionChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Upsert(view) => write!(f, "Upsert({})", view.schedule_id),
            Self::Delete(id) => write!(f, "Delete({id})"),
        }
    }
}

#[derive(Debug)]
enum ScheduleTransitionError {
    MismatchedEventScheduleId { stream_id: String, schedule_id: String },
    MalformedEvent { context: &'static str },
    CannotAddExistingSchedule { id: String },
    CannotAddDeletedSchedule { id: String },
    MissingScheduleForStateChange { id: String },
    DeletedScheduleForStateChange { id: String },
    DeletedScheduleForRemoval { id: String },
}

const fn initial_state() -> ScheduleStreamState {
    ScheduleStreamState::Initial
}

fn apply(
    stream_id: &str,
    state: ScheduleStreamState,
    event: &v1::ScheduleEvent,
) -> Result<ScheduleStreamState, ScheduleTransitionError> {
    validate_event_payload_schedule_id(stream_id, event)?;

    match (state, &event.event) {
        (ScheduleStreamState::Initial, Some(ScheduleEventCase::ScheduleCreated(inner))) => {
            Ok(ScheduleStreamState::Present(apply_schedule_created(inner)?))
        }
        (
            ScheduleStreamState::Initial,
            Some(
                ScheduleEventCase::SchedulePaused(_)
                | ScheduleEventCase::ScheduleResumed(_)
                | ScheduleEventCase::ScheduleOccurrenceRecorded(_)
                | ScheduleEventCase::ScheduleOccurrenceScheduled(_)
                | ScheduleEventCase::ScheduleCompleted(_),
            ),
        ) => Err(ScheduleTransitionError::MissingScheduleForStateChange {
            id: stream_id.to_string(),
        }),
        (ScheduleStreamState::Initial, Some(ScheduleEventCase::ScheduleRemoved(_))) => {
            Ok(ScheduleStreamState::Deleted(stream_id.to_string()))
        }
        (ScheduleStreamState::Present(view), Some(ScheduleEventCase::ScheduleCreated(_))) => {
            Err(ScheduleTransitionError::CannotAddExistingSchedule { id: view.schedule_id })
        }
        (ScheduleStreamState::Present(mut view), Some(ScheduleEventCase::SchedulePaused(_))) => {
            view.status = MessageField::some(status_proto(false));
            // Pause retains any pending occurrence: it is durable progress the
            // command-side state keeps while disabled. Resume is the boundary that
            // discards an unrecorded paused wakeup (see the `ScheduleResumed` arm).
            Ok(ScheduleStreamState::Present(view))
        }
        (ScheduleStreamState::Present(mut view), Some(ScheduleEventCase::ScheduleResumed(_))) => {
            view.status = MessageField::some(status_proto(true));
            // Resume discards the unrecorded paused wakeup so scheduling can re-arm
            // from durable occurrence progress, mirroring the command-side state.
            view.next_occurrence_at = MessageField::none();
            Ok(ScheduleStreamState::Present(view))
        }
        (ScheduleStreamState::Present(view), Some(ScheduleEventCase::ScheduleRemoved(_))) => {
            Ok(ScheduleStreamState::Deleted(view.schedule_id))
        }
        (ScheduleStreamState::Present(mut view), Some(ScheduleEventCase::ScheduleCompleted(_))) => {
            view.completed = Some(true);
            // Nothing more will fire, so there is no pending occurrence.
            view.next_occurrence_at = MessageField::none();
            Ok(ScheduleStreamState::Present(view))
        }
        (ScheduleStreamState::Present(mut view), Some(ScheduleEventCase::ScheduleOccurrenceScheduled(inner))) => {
            view.next_occurrence_at = inner.occurrence_at.clone();
            Ok(ScheduleStreamState::Present(view))
        }
        (ScheduleStreamState::Present(mut view), Some(ScheduleEventCase::ScheduleOccurrenceRecorded(inner))) => {
            view.last_occurrence_at = inner.occurrence_at.clone();
            // The pending occurrence has now fired; clear it until the next is armed.
            view.next_occurrence_at = MessageField::none();
            Ok(ScheduleStreamState::Present(view))
        }
        (ScheduleStreamState::Deleted(id), Some(ScheduleEventCase::ScheduleCreated(_))) => {
            Err(ScheduleTransitionError::CannotAddDeletedSchedule { id })
        }
        (
            ScheduleStreamState::Deleted(id),
            Some(ScheduleEventCase::SchedulePaused(_) | ScheduleEventCase::ScheduleResumed(_)),
        ) => Err(ScheduleTransitionError::DeletedScheduleForStateChange { id }),
        (ScheduleStreamState::Deleted(id), Some(ScheduleEventCase::ScheduleRemoved(_))) => {
            Err(ScheduleTransitionError::DeletedScheduleForRemoval { id })
        }
        (
            ScheduleStreamState::Deleted(id),
            Some(
                ScheduleEventCase::ScheduleOccurrenceRecorded(_)
                | ScheduleEventCase::ScheduleOccurrenceScheduled(_)
                | ScheduleEventCase::ScheduleCompleted(_),
            ),
        ) => Err(ScheduleTransitionError::DeletedScheduleForStateChange { id }),
        (_, None) => Err(ScheduleTransitionError::MalformedEvent {
            context: "schedule event has no supported case",
        }),
    }
}

/// Applies a `ScheduleCreated` event into the stored view. The event carries the
/// schedule/delivery/message definitions as `v1` protos, which are folded into
/// the read model's own `projections_v1` copies (see [`twin`]) and stamped with
/// the initial folded fields.
fn apply_schedule_created(
    created: &v1::ScheduleCreated,
) -> Result<projections_v1::ScheduleProjection, ScheduleTransitionError> {
    let Some(status) = created.status.clone().into_option() else {
        return Err(ScheduleTransitionError::MalformedEvent {
            context: "created event has no status",
        });
    };
    let Some(schedule) = created.schedule.clone().into_option() else {
        return Err(ScheduleTransitionError::MalformedEvent {
            context: "created event has no schedule",
        });
    };
    let Some(delivery) = created.delivery.clone().into_option() else {
        return Err(ScheduleTransitionError::MalformedEvent {
            context: "created event has no delivery",
        });
    };
    let Some(message) = created.message.clone().into_option() else {
        return Err(ScheduleTransitionError::MalformedEvent {
            context: "created event has no message",
        });
    };
    Ok(projections_v1::ScheduleProjection {
        schedule_id: created.schedule_id.clone(),
        status: MessageField::some(twin::status_to_projection(status)),
        completed: Some(false),
        next_occurrence_at: MessageField::none(),
        last_occurrence_at: MessageField::none(),
        schedule: MessageField::some(twin::schedule_to_projection(schedule)),
        delivery: MessageField::some(twin::delivery_to_projection(delivery)),
        message: MessageField::some(twin::message_to_projection(message)),
    })
}

/// A projection `ScheduleStatus`: `enabled` selects scheduled vs paused.
fn status_proto(enabled: bool) -> projections_v1::ScheduleStatus {
    let kind = if enabled {
        projections_v1::schedule_status::Scheduled {}.into()
    } else {
        projections_v1::schedule_status::Paused {}.into()
    };
    projections_v1::ScheduleStatus { kind: Some(kind) }
}

/// Folds the `v1` event protos into the read model's own `projections_v1` copies.
///
/// The read model owns its storage shape (it does not embed the event protos), so
/// the schedule spec, delivery, message, and status each map field-for-field into
/// their projection twin. Only the inbound direction exists here: the projection
/// is write-only, and the query side decodes the stored proto into its own value
/// objects rather than back into `v1`.
mod twin {
    use buffa::MessageField;

    use crate::{projections_v1, v1};

    use projections_v1::__buffa::oneof::delivery::Kind as ViewDeliveryKind;
    use projections_v1::__buffa::oneof::delivery::nats_message::source::Kind as ViewSourceKind;
    use projections_v1::__buffa::oneof::schedule::Kind as ViewScheduleKind;
    use projections_v1::__buffa::oneof::schedule_status::Kind as ViewStatusKind;
    use v1::__buffa::oneof::delivery::Kind as EventDeliveryKind;
    use v1::__buffa::oneof::delivery::nats_message::source::Kind as EventSourceKind;
    use v1::__buffa::oneof::schedule::Kind as EventScheduleKind;
    use v1::__buffa::oneof::schedule_status::Kind as EventStatusKind;

    pub(super) fn status_to_projection(value: v1::ScheduleStatus) -> projections_v1::ScheduleStatus {
        projections_v1::ScheduleStatus {
            kind: value.kind.map(|kind| match kind {
                EventStatusKind::Scheduled(_) => {
                    ViewStatusKind::Scheduled(Box::new(projections_v1::schedule_status::Scheduled {}))
                }
                EventStatusKind::Paused(_) => {
                    ViewStatusKind::Paused(Box::new(projections_v1::schedule_status::Paused {}))
                }
            }),
        }
    }

    pub(super) fn schedule_to_projection(value: v1::Schedule) -> projections_v1::Schedule {
        projections_v1::Schedule {
            kind: value.kind.map(|kind| match kind {
                EventScheduleKind::At(at) => ViewScheduleKind::At(Box::new(projections_v1::schedule::At { at: at.at })),
                EventScheduleKind::Every(every) => {
                    ViewScheduleKind::Every(Box::new(projections_v1::schedule::Every { every: every.every }))
                }
                EventScheduleKind::Cron(cron) => ViewScheduleKind::Cron(Box::new(projections_v1::schedule::Cron {
                    expr: cron.expr,
                    timezone: cron.timezone,
                })),
                EventScheduleKind::Rrule(rrule) => ViewScheduleKind::Rrule(Box::new(projections_v1::schedule::RRule {
                    dtstart: rrule.dtstart,
                    rrule: rrule.rrule,
                    timezone: rrule.timezone,
                    rdate: rrule.rdate,
                    exdate: rrule.exdate,
                })),
            }),
        }
    }

    pub(super) fn delivery_to_projection(value: v1::Delivery) -> projections_v1::Delivery {
        projections_v1::Delivery {
            kind: value.kind.map(|kind| match kind {
                EventDeliveryKind::NatsMessage(nats) => {
                    ViewDeliveryKind::NatsMessage(Box::new(projections_v1::delivery::NatsMessage {
                        subject: nats.subject,
                        ttl: nats.ttl,
                        source: match nats.source.into_option() {
                            Some(source) => MessageField::some(projections_v1::delivery::nats_message::Source {
                                kind: source.kind.map(|kind| match kind {
                                    EventSourceKind::LatestFromSubject(latest) => ViewSourceKind::LatestFromSubject(
                                        Box::new(projections_v1::delivery::nats_message::LatestFromSubject {
                                            subject: latest.subject,
                                        }),
                                    ),
                                }),
                            }),
                            None => MessageField::none(),
                        },
                    }))
                }
            }),
        }
    }

    pub(super) fn message_to_projection(value: v1::Message) -> projections_v1::Message {
        projections_v1::Message {
            content: value.content,
            headers: value
                .headers
                .into_iter()
                .map(|header| projections_v1::Header {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
        }
    }
}

fn validate_event_payload_schedule_id(
    stream_id: &str,
    event: &v1::ScheduleEvent,
) -> Result<(), ScheduleTransitionError> {
    let Some(schedule_id) = event_schedule_id(event) else {
        return Ok(());
    };
    if schedule_id == stream_id {
        Ok(())
    } else {
        Err(ScheduleTransitionError::MismatchedEventScheduleId {
            stream_id: stream_id.to_string(),
            schedule_id: schedule_id.to_string(),
        })
    }
}

fn event_schedule_id(event: &v1::ScheduleEvent) -> Option<&str> {
    match &event.event {
        Some(ScheduleEventCase::ScheduleCreated(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::SchedulePaused(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleResumed(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleRemoved(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleOccurrenceRecorded(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleOccurrenceScheduled(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleCompleted(inner)) => Some(&inner.schedule_id),
        None => None,
    }
}

fn projection_change(before: &ScheduleStreamState, after: &ScheduleStreamState) -> Option<ProjectionChange> {
    match (before, after) {
        (_, ScheduleStreamState::Present(view)) => Some(ProjectionChange::Upsert(view.clone())),
        // Emit a delete for any transition into Deleted, including from Initial.
        // If a ScheduleCreated was purged from the stream but a stale KV entry
        // survives, the ScheduleRemoved replayed from Initial must still clear it.
        // A KV delete on an absent key is idempotent, so this is always safe.
        (_, ScheduleStreamState::Deleted(id)) => Some(ProjectionChange::Delete(id.clone())),
        (ScheduleStreamState::Present(view), ScheduleStreamState::Initial) => {
            Some(ProjectionChange::Delete(view.schedule_id.clone()))
        }
        (ScheduleStreamState::Initial, ScheduleStreamState::Initial)
        | (ScheduleStreamState::Deleted(_), ScheduleStreamState::Initial) => None,
    }
}

impl From<projections_v1::ScheduleProjection> for ScheduleStreamState {
    fn from(view: projections_v1::ScheduleProjection) -> Self {
        Self::Present(view)
    }
}

impl std::fmt::Display for ScheduleTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MismatchedEventScheduleId { stream_id, schedule_id } => {
                write!(
                    f,
                    "schedule event id '{schedule_id}' does not match stream id '{stream_id}'"
                )
            }
            Self::MalformedEvent { context } => write!(f, "schedule event is malformed: {context}"),
            Self::CannotAddExistingSchedule { id } => write!(f, "job '{id}' already exists"),
            Self::CannotAddDeletedSchedule { id } => {
                write!(f, "job '{id}' was deleted and cannot be added again")
            }
            Self::MissingScheduleForStateChange { id } => {
                write!(f, "missing job for state change '{id}'")
            }
            Self::DeletedScheduleForStateChange { id } => {
                write!(f, "deleted schedule '{id}' cannot change state")
            }
            Self::DeletedScheduleForRemoval { id } => {
                write!(f, "job '{id}' was already deleted")
            }
        }
    }
}

impl std::error::Error for ScheduleTransitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

pub(crate) async fn catch_up_schedules_read_model<J>(js: &J) -> Result<(), SchedulerError>
where
    J: JetStreamCreateKeyValue<Store = kv::Store>
        + JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream: jetstream::stream::Stream = open_events_stream(js).await?;
    let info = stream.get_info().await.map_err(|source| {
        SchedulerError::event_source(
            "failed to query events stream info for schedules read-model catch-up",
            source,
        )
    })?;
    if info.state.messages == 0 {
        return Ok(());
    }

    let bucket = get_or_create_schedules_bucket(js).await?;
    let checkpoint = read_read_model_checkpoint(&bucket).await?;
    if checkpoint >= info.state.last_sequence {
        return Ok(());
    }

    // Rebuild the read model by folding the full event history into a fresh
    // state. Folding from empty (rather than the live KV) keeps replay
    // idempotent: re-running yields the same projection no matter how far a
    // previous attempt got, and events arrive in per-schedule order so the
    // state machine never sees an out-of-order transition.
    //
    // A single malformed/foreign/undecodable event must never wedge startup, so
    // per-event anomalies are logged and skipped. Only genuine infrastructure
    // failures (message read, KV write) abort the rebuild; the checkpoint is then
    // left behind so the next start re-folds and self-heals.
    let mut states = BTreeMap::new();

    let consumer = stream
        .create_consumer(event_replay_consumer_config(info.state.first_sequence))
        .await
        .map_err(|source| {
            SchedulerError::event_source("failed to create schedules read-model catch-up consumer", source)
        })?;
    let mut messages = consumer.messages().await.map_err(|source| {
        SchedulerError::event_source("failed to open schedules read-model catch-up stream", source)
    })?;

    // Drain to the tail, then re-check the stream: events appended while folding
    // (e.g. another instance during a rolling restart) advance the target so they
    // are never stranded between catch-up's ceiling and the live path.
    let mut target = info.state.last_sequence;
    // A KV write failure (transient backend error, or a permanently un-writable
    // key) must not wedge startup. Track it and only advance the checkpoint when
    // the fold was fully clean, so a later start re-folds and repairs rather than
    // declaring an incomplete rebuild complete.
    let mut clean = true;
    // The checkpoint may only advance once the fold has actually reached `target`.
    // The message stream can end early (returning `None`) before the tail is
    // drained; without this flag the post-loop code would still checkpoint at
    // `target` and permanently skip the unfolded `(last folded, target]` gap.
    let mut reached_target = false;

    while let Some(message) = messages.next().await {
        let message = message.map_err(|source| {
            SchedulerError::event_source(
                "failed to read schedule event during schedules read-model catch-up",
                source,
            )
        })?;
        let sequence = event_message_sequence(&message, "failed to read schedules read-model catch-up event metadata")?;
        if let Err(source) = fold_catch_up_message(&bucket, &mut states, &message).await {
            tracing::warn!(stream_sequence = sequence, %source, "failed to write a projected schedule during catch-up; will re-fold on next start");
            clean = false;
        }

        if sequence >= target {
            let fresh = stream.get_info().await.map_err(|source| {
                SchedulerError::event_source("failed to re-query events stream info during catch-up", source)
            })?;
            if fresh.state.last_sequence > target {
                target = fresh.state.last_sequence;
                continue;
            }
            reached_target = true;
            break;
        }
    }

    if !clean || !reached_target {
        // The rebuild is incomplete (a KV write failed, or the stream ended before
        // the tail). Leave the checkpoint behind so the next start re-folds and
        // self-heals rather than declaring catch-up complete with a gap.
        if !reached_target {
            tracing::warn!(
                target,
                "schedules read-model catch-up ended before reaching the stream tail; will re-fold on next start"
            );
        }
        return Ok(());
    }

    // Reconcile the bucket against the freshly folded state. This catch-up replays
    // the full event log from empty, so `states` is authoritative for which
    // schedules should have a row. Deleting every current entry that is not one of
    // them removes both pre-v2 (raw schedule id) rows and any stale row whose
    // updating events were skipped during the fold, so a clean rebuild never leaves
    // an outdated projection behind.
    let live_keys: HashSet<String> = states
        .values()
        .filter_map(|state| match state {
            ScheduleStreamState::Present(view) => Some(read_model_key(&view.schedule_id)),
            ScheduleStreamState::Initial | ScheduleStreamState::Deleted(_) => None,
        })
        .collect();
    reconcile_read_model_keys(&bucket, &live_keys).await?;

    write_read_model_checkpoint(&bucket, target).await
}

/// Reconciles the bucket to the freshly folded state: deletes every entry that is
/// neither the checkpoint nor one of `live_keys` (the derived keys of the
/// schedules the rebuild folded as present).
///
/// Because catch-up replays the full event log from empty, `live_keys` is the
/// authoritative set of rows that should exist. Deleting the rest removes pre-v2
/// raw-id rows and any stale row whose updating events were skipped during the
/// fold, so a clean rebuild can never leave an outdated projection behind.
///
/// This trades the earlier shape-only conservatism for correctness, so it relies
/// on the single-active-writer invariant (see the module docs): with a concurrent
/// writer — a misconfigured rolling restart — it could delete a row a peer just
/// created, which that peer's next event or restart re-creates.
async fn reconcile_read_model_keys(bucket: &kv::Store, live_keys: &HashSet<String>) -> Result<(), SchedulerError> {
    let mut keys = bucket.keys().await.map_err(|source| {
        SchedulerError::kv_source("failed to list schedules read-model keys for reconcile", source)
    })?;
    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| {
            SchedulerError::kv_source("failed to read schedules read-model key for reconcile", source)
        })?;
        if key == SCHEDULES_CHECKPOINT_KEY || live_keys.contains(&key) {
            continue;
        }
        bucket
            .delete(key.clone())
            .await
            .map_err(|source| SchedulerError::kv_source("failed to delete stale read-model key", source))?;
        tracing::warn!(%key, "deleted stale schedules read-model entry during catch-up reconcile");
    }

    Ok(())
}

/// Folds one catch-up message into `states`, writing any resulting KV change.
///
/// Per-event anomalies (undecodable payload, foreign subject, mismatched id, an
/// invalid transition) are logged and skipped — they must not abort the rebuild.
/// Only a KV write failure is propagated, because it is transient infrastructure
/// and re-folding on the next start repairs it.
async fn fold_catch_up_message(
    bucket: &kv::Store,
    states: &mut BTreeMap<String, ScheduleStreamState>,
    message: &jetstream::Message,
) -> Result<(), SchedulerError> {
    let event = match decode_recorded_delivery_message(message) {
        Ok(event) => event,
        Err(source) => {
            tracing::warn!(%source, "skipping undecodable schedule event during read-model catch-up");
            return Ok(());
        }
    };
    let decoded = match event.decode::<v1::ScheduleEvent>() {
        Ok(decoded) => decoded,
        Err(source) => {
            tracing::warn!(%source, "skipping unparseable schedule event during read-model catch-up");
            return Ok(());
        }
    };
    let Some(decoded) = decoded.into_decoded() else {
        // A foreign or newer-than-this-deploy event type: not part of this
        // read model, skip without disturbing state.
        return Ok(());
    };
    let subject_token = match read_model_token_from_event_subject(event.stream_id()) {
        Ok(token) => token,
        Err(source) => {
            tracing::warn!(%source, "skipping schedule event with unrecognized subject during read-model catch-up");
            return Ok(());
        }
    };
    // The subject carries the schedule's derived routing token, not the raw id;
    // the raw id lives in the event payload. Recover it from the payload and
    // confirm it routes to this subject, so a misrouted event can never fold into
    // another schedule's view — and so `apply`'s id check matches the stream id
    // instead of rejecting every replayed event.
    let Some(schedule_id) = event_schedule_id(&decoded) else {
        tracing::warn!(%subject_token, "skipping schedule event without a payload schedule id during read-model catch-up");
        return Ok(());
    };
    if read_model_key(schedule_id) != subject_token {
        tracing::warn!(
            %subject_token,
            %schedule_id,
            "skipping schedule event whose payload id does not route to its subject during read-model catch-up"
        );
        return Ok(());
    }
    let change = match apply_event_to_read_model_state(states, schedule_id, &decoded) {
        Ok(change) => change,
        Err(source) => {
            tracing::warn!(%schedule_id, %source, "skipping invalid schedule transition during read-model catch-up");
            return Ok(());
        }
    };
    if let Some(change) = change {
        apply_projection_change(bucket, &change).await?;
    }
    Ok(())
}

pub(crate) async fn project_appended_events(
    bucket: &kv::Store,
    job_id: &str,
    events: &[Event],
    final_position: StreamPosition,
) -> Result<(), SchedulerError> {
    if events.is_empty() {
        return Ok(());
    }

    let mut states = BTreeMap::new();
    if let Some(view) = read_projected_view(bucket, job_id).await? {
        states.insert(job_id.to_string(), ScheduleStreamState::from(view));
    }

    for event in events {
        let decoded = v1::ScheduleEvent::decode(EventData::new(&event.r#type, &event.content)).map_err(|source| {
            SchedulerError::event_source("failed to decode schedule event for schedules read model", source)
        })?;
        let Some(decoded) = decoded.into_decoded() else {
            continue;
        };
        if let Some(change) = apply_event_to_read_model_state(&mut states, job_id, &decoded)? {
            apply_projection_change(bucket, &change).await?;
        }
    }
    maybe_advance_read_model_checkpoint(bucket, final_position.as_u64(), events.len() as u64).await
}

fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<StreamEvent, SchedulerError> {
    let stream_id = message.subject.to_string();
    record_stream_message(message, stream_id)
        .map_err(|source| SchedulerError::event_source("failed to decode stored schedule event", source))
}

fn decode_recorded_delivery_message(message: &async_nats::jetstream::Message) -> Result<StreamEvent, SchedulerError> {
    // A consumer-delivered message carries its stream sequence and timestamp in
    // its JetStream metadata, not in the direct-get headers that
    // `StreamMessage::try_from` expects, so build the stream message from
    // `info()` rather than reparsing headers that are absent here.
    let info = message
        .info()
        .map_err(|source| SchedulerError::event_source("failed to read schedule event delivery metadata", source))?;
    let stream_message = async_nats::jetstream::message::StreamMessage {
        subject: message.subject.clone(),
        sequence: info.stream_sequence,
        headers: message.headers.clone().unwrap_or_default(),
        payload: message.payload.clone(),
        time: info.published,
    };

    decode_recorded_job_event(stream_message)
}

fn event_replay_consumer_config(start_sequence: u64) -> pull::OrderedConfig {
    pull::OrderedConfig {
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        replay_policy: ReplayPolicy::Instant,
        // Only schedule-event subjects; if the stream ever carries another
        // aggregate's subjects, they must not reach this projection.
        filter_subject: EVENTS_SUBJECT_PATTERN.to_string(),
        ..Default::default()
    }
}

fn event_message_sequence(message: &jetstream::Message, context: &'static str) -> Result<u64, SchedulerError> {
    message
        .info()
        .map(|info| info.stream_sequence)
        .map_err(|source| SchedulerError::event_source(context, source))
}

/// Reads the prior stored view for a schedule so the live path can fold new
/// events onto it. Works purely in proto; `get` returns `None` for a tombstone.
async fn read_projected_view(
    bucket: &kv::Store,
    id: &str,
) -> Result<Option<projections_v1::ScheduleProjection>, SchedulerError> {
    let Some(value) = bucket
        .get(read_model_key(id))
        .await
        .map_err(|source| SchedulerError::kv_source("failed to read projected schedule", source))?
    else {
        return Ok(None);
    };

    <projections_v1::ScheduleProjection as buffa::Message>::decode_from_slice(&value)
        .map(Some)
        .map_err(|source| SchedulerError::kv_source("failed to decode projected schedule view", source))
}

async fn read_read_model_checkpoint(bucket: &kv::Store) -> Result<u64, SchedulerError> {
    let Some(value) = bucket
        .get(SCHEDULES_CHECKPOINT_KEY.to_string())
        .await
        .map_err(|source| SchedulerError::kv_source("failed to read schedules read-model checkpoint", source))?
    else {
        return Ok(0);
    };

    // A corrupt checkpoint value (truncated write, manual edit, non-UTF8) must
    // not wedge startup. Treat it as 0 and rebuild from the beginning; catch-up
    // is idempotent, so a full re-fold is always safe and self-healing.
    match String::from_utf8(value.to_vec())
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        Some(sequence) => Ok(sequence),
        None => {
            tracing::warn!(
                key = SCHEDULES_CHECKPOINT_KEY,
                "schedules read-model checkpoint is unreadable; treating as 0 and rebuilding"
            );
            Ok(0)
        }
    }
}

async fn write_read_model_checkpoint(bucket: &kv::Store, sequence: u64) -> Result<(), SchedulerError> {
    bucket
        .put(SCHEDULES_CHECKPOINT_KEY.to_string(), sequence.to_string().into())
        .await
        .map(|_| ())
        .map_err(|source| SchedulerError::kv_source("failed to write schedules read-model checkpoint", source))
}

async fn maybe_advance_read_model_checkpoint(
    bucket: &kv::Store,
    final_position: u64,
    appended: u64,
) -> Result<(), SchedulerError> {
    // An append commits `appended` events with consecutive sequences ending at
    // `final_position`, so the event just before the block is at
    // `final_position - appended`. Advance only when the checkpoint sits exactly
    // there, keeping it a contiguous low-watermark; otherwise leave it for
    // catch-up to rebuild from. Correctness never depends on this advancing —
    // it only lets a restart skip the re-fold when nothing is missing.
    let current = read_read_model_checkpoint(bucket).await?;
    if current != final_position.saturating_sub(appended) {
        return Ok(());
    }

    write_read_model_checkpoint(bucket, final_position).await
}

async fn apply_projection_change(kv: &kv::Store, change: &ProjectionChange) -> Result<(), SchedulerError> {
    match change {
        ProjectionChange::Upsert(view) => {
            let value = buffa::Message::encode_to_vec(view);
            kv.put(read_model_key(&view.schedule_id), value.into())
                .await
                .map_err(|source| SchedulerError::kv_source("failed to store projected job state", source))?;
        }
        ProjectionChange::Delete(id) => {
            kv.delete(read_model_key(id))
                .await
                .map_err(|source| SchedulerError::kv_source("failed to delete projected job state", source))?;
        }
    }

    Ok(())
}

fn apply_event_to_read_model_state(
    states: &mut BTreeMap<String, ScheduleStreamState>,
    stream_id: &str,
    event: &v1::ScheduleEvent,
) -> Result<Option<ProjectionChange>, SchedulerError> {
    let current_state = states.get(stream_id).cloned().unwrap_or_else(initial_state);
    let next_state = apply(stream_id, current_state.clone(), event).map_err(|source| {
        SchedulerError::event_source("failed to apply schedule event to schedules read model", source)
    })?;
    let change = projection_change(&current_state, &next_state);

    match next_state {
        ScheduleStreamState::Present(_) | ScheduleStreamState::Deleted(_) => {
            states.insert(stream_id.to_string(), next_state);
        }
        ScheduleStreamState::Initial => {
            states.remove(stream_id);
        }
    }

    Ok(change)
}

/// Extracts a schedule event subject's derived routing token — the 32-hex
/// `key.simple()` the publisher appends as the subject's last segment.
///
/// The subject is `EVENTS_SUBJECT_PREFIX` then `.` then the derived key (never the
/// raw schedule id, so this cannot recover the id; the raw id is read from the
/// event payload). The token is used only to confirm an event routes to the
/// schedule named in its payload before that event is folded.
fn read_model_token_from_event_subject(subject: &str) -> Result<String, SchedulerError> {
    // Confirm the subject is a schedule event, then take its final dot-segment as
    // the key. `key.simple()` never contains a dot, so the last segment is always
    // the whole token — correct whether or not `EVENTS_SUBJECT_PREFIX` itself ends
    // in a dot (stripping a prefix without the trailing dot would otherwise leave a
    // leading `.` that never matches the derived KV key).
    let rest = subject.strip_prefix(EVENTS_SUBJECT_PREFIX).ok_or_else(|| {
        SchedulerError::event_source(
            "failed to derive schedule routing token from event subject",
            std::io::Error::other(subject.to_string()),
        )
    })?;
    let token = rest.rsplit('.').next().unwrap_or(rest);
    if token.is_empty() {
        return Err(SchedulerError::event_source(
            "schedule event subject has an empty routing token",
            std::io::Error::other(subject.to_string()),
        ));
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;
    use buffa_types::google::protobuf::{Duration, Timestamp};
    use chrono::DateTime;

    use super::*;
    use crate::v1;

    use projections_v1::__buffa::oneof::schedule_status::Kind as ViewStatusKind;

    fn timestamp_from_str(rfc3339: &str) -> Timestamp {
        let dt = DateTime::parse_from_rfc3339(rfc3339).unwrap();
        Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
            ..Default::default()
        }
    }

    fn proto_job_created(id: &str) -> v1::ScheduleCreated {
        v1::ScheduleCreated {
            schedule_id: id.to_string(),
            status: MessageField::some(v1::ScheduleStatus {
                kind: Some(v1::schedule_status::Scheduled {}.into()),
            }),
            schedule: MessageField::some(v1::Schedule {
                kind: Some(
                    v1::schedule::Every {
                        every: MessageField::some(Duration {
                            seconds: 30,
                            ..Default::default()
                        }),
                    }
                    .into(),
                ),
            }),
            delivery: MessageField::some(v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: "agent.run".to_string(),
                        ttl: MessageField::none(),
                        source: MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: MessageField::some(v1::Message {
                content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
                    content_type: "application/json".to_string(),
                    data: r#"{"kind":"heartbeat"}"#.as_bytes().to_vec(),
                }),
                headers: Vec::new(),
            }),
        }
    }

    fn added_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(proto_job_created(id).into()),
        }
    }

    fn paused_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn removed_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn completed_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCompleted {
                    schedule_id: id.to_string(),
                    last_occurrence_sequence: Some(2),
                }
                .into(),
            ),
        }
    }

    fn occurrence_scheduled_event(id: &str, at: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceScheduled {
                    schedule_id: id.to_string(),
                    occurrence_sequence: Some(1),
                    occurrence_at: MessageField::some(timestamp_from_str(at)),
                    scheduled_at: MessageField::some(timestamp_from_str(at)),
                }
                .into(),
            ),
        }
    }

    fn occurrence_recorded_event(id: &str, sequence: u64, at: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: id.to_string(),
                    occurrence_sequence: Some(sequence),
                    occurrence_at: MessageField::some(timestamp_from_str(at)),
                    recorded_at: MessageField::some(timestamp_from_str(at)),
                }
                .into(),
            ),
        }
    }

    fn present(state: ScheduleStreamState) -> projections_v1::ScheduleProjection {
        match state {
            ScheduleStreamState::Present(view) => view,
            other => panic!("expected present schedule, got {other:?}"),
        }
    }

    fn is_paused(view: &projections_v1::ScheduleProjection) -> bool {
        matches!(
            view.status.as_option().and_then(|status| status.kind.as_ref()),
            Some(ViewStatusKind::Paused(_))
        )
    }

    #[test]
    fn created_event_copies_the_event_definition_into_the_view() {
        let created = proto_job_created("backup");
        let view = present(apply("backup", initial_state(), &added_event("backup")).unwrap());

        assert_eq!(view.schedule_id, "backup");
        assert_eq!(view.completed, Some(false));
        assert!(view.next_occurrence_at.as_option().is_none());
        // The definition fields are folded field-for-field from the event into
        // the read model's own projection copies.
        assert_eq!(
            view.schedule.into_option(),
            created.schedule.into_option().map(twin::schedule_to_projection)
        );
        assert_eq!(
            view.delivery.into_option(),
            created.delivery.into_option().map(twin::delivery_to_projection)
        );
        assert_eq!(
            view.message.into_option(),
            created.message.into_option().map(twin::message_to_projection)
        );
    }

    #[test]
    fn pause_then_resume_toggles_status() {
        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let paused = present(apply("backup", created, &paused_event("backup")).unwrap());
        assert!(is_paused(&paused));

        let resumed = present(
            apply(
                "backup",
                ScheduleStreamState::Present(paused),
                &v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleResumed {
                            schedule_id: "backup".to_string(),
                        }
                        .into(),
                    ),
                },
            )
            .unwrap(),
        );
        assert!(!is_paused(&resumed));
    }

    #[test]
    fn pause_retains_and_resume_clears_the_pending_next_occurrence() {
        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let scheduled = present(
            apply(
                "backup",
                created,
                &occurrence_scheduled_event("backup", "2026-06-04T00:00:00+00:00"),
            )
            .unwrap(),
        );
        assert!(scheduled.next_occurrence_at.as_option().is_some());

        // Pause keeps the pending occurrence (durable progress retained while disabled).
        let paused = present(
            apply(
                "backup",
                ScheduleStreamState::Present(scheduled),
                &paused_event("backup"),
            )
            .unwrap(),
        );
        assert!(is_paused(&paused));
        assert!(
            paused.next_occurrence_at.as_option().is_some(),
            "pausing retains the pending occurrence"
        );

        // Resume discards the unrecorded paused wakeup so scheduling can re-arm.
        let resumed = present(
            apply(
                "backup",
                ScheduleStreamState::Present(paused),
                &v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleResumed {
                            schedule_id: "backup".to_string(),
                        }
                        .into(),
                    ),
                },
            )
            .unwrap(),
        );
        assert!(!is_paused(&resumed));
        assert!(
            resumed.next_occurrence_at.as_option().is_none(),
            "resuming clears the pending occurrence"
        );
    }

    #[test]
    fn event_projection_replays_latest_state() {
        let mut state = initial_state();
        for event in [added_event("backup"), paused_event("backup"), removed_event("backup")] {
            state = apply("backup", state, &event).unwrap();
        }
        assert_eq!(state, ScheduleStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn completed_event_marks_completed_without_removing() {
        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let view = present(apply("backup", created, &completed_event("backup")).unwrap());
        assert_eq!(view.completed, Some(true));
        assert!(view.next_occurrence_at.as_option().is_none());
    }

    #[test]
    fn tracks_next_and_last_occurrence() {
        let at = timestamp_from_str("2026-06-04T00:00:00+00:00");

        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let scheduled = present(
            apply(
                "backup",
                created,
                &occurrence_scheduled_event("backup", "2026-06-04T00:00:00+00:00"),
            )
            .unwrap(),
        );
        assert_eq!(scheduled.next_occurrence_at.as_option(), Some(&at));
        assert!(scheduled.last_occurrence_at.as_option().is_none());

        let recorded = present(
            apply(
                "backup",
                ScheduleStreamState::Present(scheduled),
                &occurrence_recorded_event("backup", 1, "2026-06-04T00:00:00+00:00"),
            )
            .unwrap(),
        );
        assert_eq!(recorded.last_occurrence_at.as_option(), Some(&at));
        assert!(
            recorded.next_occurrence_at.as_option().is_none(),
            "recording consumes the pending occurrence"
        );
    }

    #[test]
    fn rejects_recreating_a_deleted_schedule() {
        let error = apply(
            "backup",
            ScheduleStreamState::Deleted("backup".to_string()),
            &added_event("backup"),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::CannotAddDeletedSchedule { .. }
        ));
    }

    #[test]
    fn state_change_requires_an_existing_schedule() {
        let error = apply("missing", initial_state(), &paused_event("missing")).unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::MissingScheduleForStateChange { .. }
        ));
    }

    #[test]
    fn rejects_recreating_an_existing_schedule() {
        let created = apply("backup", initial_state(), &added_event("backup")).unwrap();
        let error = apply("backup", created, &added_event("backup")).unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::CannotAddExistingSchedule { .. }
        ));
    }

    #[test]
    fn initial_removal_creates_a_deleted_tombstone() {
        let state = apply("backup", initial_state(), &removed_event("backup")).unwrap();
        assert_eq!(state, ScheduleStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn mismatched_payload_id_is_rejected() {
        let error = apply("alpha", initial_state(), &added_event("beta")).unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::MismatchedEventScheduleId { .. }
        ));
    }

    #[test]
    fn projection_change_upserts_then_deletes() {
        let before = initial_state();
        let after = apply("backup", before.clone(), &added_event("backup")).unwrap();
        assert!(matches!(
            projection_change(&before, &after),
            Some(ProjectionChange::Upsert(_))
        ));

        let removed = apply("backup", after.clone(), &removed_event("backup")).unwrap();
        match projection_change(&after, &removed) {
            Some(ProjectionChange::Delete(id)) => assert_eq!(id, "backup"),
            other => panic!("expected delete, got {other:?}"),
        }
    }

    #[test]
    fn read_model_state_rejects_recreating_deleted_schedule() {
        let mut states = BTreeMap::new();
        let id = "alpha".to_string();
        apply_event_to_read_model_state(&mut states, &id, &added_event("alpha")).unwrap();
        apply_event_to_read_model_state(&mut states, &id, &removed_event("alpha")).unwrap();
        let error = apply_event_to_read_model_state(&mut states, &id, &added_event("alpha")).unwrap_err();
        assert!(error.to_string().contains("deleted"));
        assert_eq!(states.get("alpha"), Some(&ScheduleStreamState::Deleted(id)));
    }

    #[test]
    fn round_trips_through_the_kv_codec() {
        // What the projection writes must decode back to an equal view.
        let view = present(apply("backup", initial_state(), &added_event("backup")).unwrap());
        let encoded = buffa::Message::encode_to_vec(&view);
        let decoded = <projections_v1::ScheduleProjection as buffa::Message>::decode_from_slice(&encoded).unwrap();
        assert_eq!(decoded, view);
    }
}
