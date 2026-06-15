use chrono::{DateTime, Utc};
use trogon_decider_runtime::StreamPosition;

use crate::commands::domain::{
    Delivery, Schedule, ScheduleEventStatus, ScheduleId, ScheduleMessage, ScheduleOccurrenceSequence,
};
use crate::processor::execution::checkpoints::{ReconcileOutcome, ScheduleCheckpointRecord, ScheduleStatus};

use super::{
    DispatchRequest, RRuleCursor, ScheduleRequest, ScheduleRequestError, ScheduleSubject, next_rrule_occurrence,
};

pub(crate) const CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE: &str = "trogon.scheduler.corrupt-checkpoint";

/// One-shot schedules past-due by no more than this window still publish: the
/// execution stream fires an `@at` in the past immediately, so a schedule that
/// is late only because of processing lag is delivered instead of silently
/// expired. Anything older (e.g. replayed history) expires without publishing.
const PAST_AT_GRACE: chrono::Duration = chrono::Duration::minutes(5);

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleDefinition {
    pub status: ScheduleEventStatus,
    pub schedule: Schedule,
    pub delivery: Delivery,
    pub message: ScheduleMessage,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleChange {
    Created {
        schedule_id: ScheduleId,
        definition: Box<ScheduleDefinition>,
    },
    Paused {
        schedule_id: ScheduleId,
    },
    Resumed {
        schedule_id: ScheduleId,
    },
    Removed {
        schedule_id: ScheduleId,
    },
    OccurrenceFired {
        schedule_id: ScheduleId,
        occurrence_sequence: ScheduleOccurrenceSequence,
        occurrence_at: DateTime<Utc>,
    },
}

impl ScheduleChange {
    pub fn schedule_id(&self) -> &ScheduleId {
        match self {
            Self::Created { schedule_id, .. }
            | Self::Paused { schedule_id }
            | Self::Resumed { schedule_id }
            | Self::Removed { schedule_id }
            | Self::OccurrenceFired { schedule_id, .. } => schedule_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileAction {
    Publish(ScheduleRequest),
    Dispatch(DispatchRequest),
    Purge(ScheduleSubject),
    Multiple(Vec<ReconcileAction>),
    CheckpointOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reconciliation {
    pub action: ReconcileAction,
    pub next_checkpoint: ScheduleCheckpointRecord,
}

#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("no scheduler checkpoint exists for schedule '{schedule_id}'")]
    MissingCheckpoint { schedule_id: ScheduleId },
    #[error("scheduler checkpoint for schedule '{schedule_id}' cannot be resumed")]
    UnrecoverableCheckpoint { schedule_id: ScheduleId },
    #[error("schedule request failed: {source}")]
    ScheduleRequest {
        #[source]
        source: ScheduleRequestError,
    },
}

pub fn reconcile(
    current: Option<&ScheduleCheckpointRecord>,
    event: &ScheduleChange,
    stream_position: StreamPosition,
    event_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Reconciliation, ReconcileError> {
    if let Some(current) = current
        && stream_position <= current.last_applied_stream_position
    {
        return Ok(Reconciliation {
            action: ReconcileAction::CheckpointOnly,
            next_checkpoint: stale_no_op(current),
        });
    }

    match event {
        ScheduleChange::Created {
            schedule_id,
            definition,
        } => {
            let ScheduleDefinition {
                status,
                schedule,
                delivery,
                message,
            } = definition.as_ref();

            let (action, schedule_status, outcome) = if *status == ScheduleEventStatus::Paused {
                (
                    ReconcileAction::CheckpointOnly,
                    ScheduleStatus::Paused,
                    ReconcileOutcome::StoredPaused,
                )
            } else {
                decide_enabled(schedule, schedule_id, delivery, message, now)?
            };

            Ok(Reconciliation {
                action,
                next_checkpoint: ScheduleCheckpointRecord {
                    schedule_id: schedule_id.clone(),
                    status: schedule_status,
                    schedule: schedule.clone(),
                    delivery: delivery.clone(),
                    message: message.clone(),
                    last_applied_stream_position: stream_position,
                    last_applied_event_id: event_id.map(str::to_string),
                    last_outcome: outcome,
                },
            })
        }
        ScheduleChange::Paused { schedule_id } => {
            purge(current, schedule_id, ScheduleStatus::Paused, stream_position, event_id)
        }
        ScheduleChange::Removed { schedule_id } => {
            purge(current, schedule_id, ScheduleStatus::Removed, stream_position, event_id)
        }
        ScheduleChange::Resumed { schedule_id } => {
            reconcile_resumed(current, schedule_id, stream_position, event_id, now)
        }
        ScheduleChange::OccurrenceFired {
            schedule_id,
            occurrence_sequence,
            occurrence_at,
        } => reconcile_occurrence_fired(
            current,
            schedule_id,
            *occurrence_sequence,
            *occurrence_at,
            stream_position,
            event_id,
        ),
    }
}

fn reconcile_occurrence_fired(
    current: Option<&ScheduleCheckpointRecord>,
    schedule_id: &ScheduleId,
    occurrence_sequence: ScheduleOccurrenceSequence,
    occurrence_at: DateTime<Utc>,
    stream_position: StreamPosition,
    event_id: Option<&str>,
) -> Result<Reconciliation, ReconcileError> {
    let Some(current) = current else {
        return Err(ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id.clone(),
        });
    };
    if current.schedule_id != schedule_id.clone() {
        return Err(ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id.clone(),
        });
    }
    if current.status != ScheduleStatus::Scheduled || !matches!(current.schedule, Schedule::RRule { .. }) {
        return Ok(Reconciliation {
            action: ReconcileAction::CheckpointOnly,
            next_checkpoint: advanced(
                current,
                current.status,
                ReconcileOutcome::DuplicateStale,
                stream_position,
                event_id,
            ),
        });
    }

    let dispatch = DispatchRequest::build_occurrence(
        schedule_id,
        occurrence_sequence,
        occurrence_at,
        &current.delivery,
        &current.message,
    )
    .map_err(|source| ReconcileError::ScheduleRequest { source })?;
    let (mut action, status, outcome) = decide_rrule(
        &current.schedule,
        &current.schedule_id,
        &current.delivery,
        &current.message,
        RRuleCursor::after(occurrence_at),
    )?;
    if outcome == ReconcileOutcome::Expired {
        action = ReconcileAction::Purge(current.subject());
    }

    Ok(Reconciliation {
        action: ReconcileAction::Multiple(vec![ReconcileAction::Dispatch(dispatch), action]),
        next_checkpoint: advanced(current, status, outcome, stream_position, event_id),
    })
}

fn reconcile_resumed(
    current: Option<&ScheduleCheckpointRecord>,
    schedule_id: &ScheduleId,
    stream_position: StreamPosition,
    event_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Reconciliation, ReconcileError> {
    let Some(current) = current else {
        return Err(ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id.clone(),
        });
    };
    if current.schedule_id != schedule_id.clone() {
        return Err(ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id.clone(),
        });
    }
    // The decider rejects Resume on a deleted schedule (`ScheduleDeleted` in
    // `commands/resume_schedule.rs`), so a Resumed event can never follow a
    // Removed one in the stream. Reaching this arm means the checkpoint
    // itself is wrong (e.g. a corrupt-recovery tombstone), not a benign
    // command race — the durable failure is intentional.
    if current.status == ScheduleStatus::Removed || is_corrupt_checkpoint_placeholder(current) {
        return Err(ReconcileError::UnrecoverableCheckpoint {
            schedule_id: schedule_id.clone(),
        });
    }

    let (action, status, outcome) = decide_enabled(
        &current.schedule,
        &current.schedule_id,
        &current.delivery,
        &current.message,
        now,
    )?;

    Ok(Reconciliation {
        action,
        next_checkpoint: advanced(current, status, outcome, stream_position, event_id),
    })
}

fn purge(
    current: Option<&ScheduleCheckpointRecord>,
    schedule_id: &ScheduleId,
    status: ScheduleStatus,
    stream_position: StreamPosition,
    event_id: Option<&str>,
) -> Result<Reconciliation, ReconcileError> {
    let Some(current) = current else {
        return Err(ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id.clone(),
        });
    };
    if current.schedule_id != schedule_id.clone() {
        return Err(ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id.clone(),
        });
    }

    Ok(Reconciliation {
        action: ReconcileAction::Purge(current.subject()),
        next_checkpoint: advanced(current, status, ReconcileOutcome::Purged, stream_position, event_id),
    })
}

fn decide_enabled(
    schedule: &Schedule,
    schedule_id: &ScheduleId,
    delivery: &Delivery,
    message: &ScheduleMessage,
    now: DateTime<Utc>,
) -> Result<(ReconcileAction, ScheduleStatus, ReconcileOutcome), ReconcileError> {
    if matches!(schedule, Schedule::RRule { .. }) {
        return decide_rrule(
            schedule,
            schedule_id,
            delivery,
            message,
            RRuleCursor::at_or_after(now - PAST_AT_GRACE),
        );
    }

    if let Schedule::At { at } = schedule
        && *at <= now - PAST_AT_GRACE
    {
        return Ok((
            ReconcileAction::CheckpointOnly,
            ScheduleStatus::Expired,
            ReconcileOutcome::Expired,
        ));
    }

    let request = ScheduleRequest::build(schedule_id, schedule, delivery, message)
        .map_err(|source| ReconcileError::ScheduleRequest { source })?;

    Ok((
        ReconcileAction::Publish(request),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    ))
}

fn decide_rrule(
    schedule: &Schedule,
    schedule_id: &ScheduleId,
    delivery: &Delivery,
    message: &ScheduleMessage,
    cursor: RRuleCursor,
) -> Result<(ReconcileAction, ScheduleStatus, ReconcileOutcome), ReconcileError> {
    let next_occurrence =
        next_rrule_occurrence(schedule, cursor).map_err(|source| ReconcileError::ScheduleRequest {
            source: ScheduleRequestError::RRuleExpansion { source },
        })?;

    let Some(at) = next_occurrence else {
        return Ok((
            ReconcileAction::CheckpointOnly,
            ScheduleStatus::Expired,
            ReconcileOutcome::Expired,
        ));
    };

    let _ = DispatchRequest::build(schedule_id, delivery, message)
        .map_err(|source| ReconcileError::ScheduleRequest { source })?;
    let request = ScheduleRequest::build_rrule_wakeup(schedule_id, at, delivery)
        .map_err(|source| ReconcileError::ScheduleRequest { source })?;

    Ok((
        ReconcileAction::Publish(request),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    ))
}

fn stale_no_op(current: &ScheduleCheckpointRecord) -> ScheduleCheckpointRecord {
    let mut next = current.clone();
    next.last_outcome = ReconcileOutcome::DuplicateStale;
    next
}

fn advanced(
    current: &ScheduleCheckpointRecord,
    status: ScheduleStatus,
    outcome: ReconcileOutcome,
    stream_position: StreamPosition,
    event_id: Option<&str>,
) -> ScheduleCheckpointRecord {
    let mut next = current.clone();
    next.status = status;
    next.last_outcome = outcome;
    next.last_applied_stream_position = stream_position;
    next.last_applied_event_id = event_id.map(str::to_string);
    next
}

fn is_corrupt_checkpoint_placeholder(current: &ScheduleCheckpointRecord) -> bool {
    match &current.delivery {
        Delivery::NatsEvent { route, .. } => route.as_str() == CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::commands::domain::{Delivery, MessageContent, ScheduleHeaders, ScheduleMessage};

    fn schedule_id(raw: &str) -> ScheduleId {
        ScheduleId::parse(raw).unwrap()
    }

    fn instant(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw).unwrap().with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        instant("2026-06-03T00:00:00Z")
    }

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).unwrap()
    }

    fn message() -> ScheduleMessage {
        ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::default(),
        }
    }

    fn created(id: &str, status: ScheduleEventStatus, schedule: Schedule) -> ScheduleChange {
        ScheduleChange::Created {
            schedule_id: schedule_id(id),
            definition: Box::new(ScheduleDefinition {
                status,
                schedule,
                delivery: Delivery::nats_event("agent.run").unwrap(),
                message: message(),
            }),
        }
    }

    fn occurrence_fired(id: &str, occurrence_sequence: u64, occurrence_at: &str) -> ScheduleChange {
        ScheduleChange::OccurrenceFired {
            schedule_id: schedule_id(id),
            occurrence_sequence: ScheduleOccurrenceSequence::try_new(occurrence_sequence).unwrap(),
            occurrence_at: instant(occurrence_at),
        }
    }

    fn scheduled_record(id: &str, schedule: Schedule) -> ScheduleCheckpointRecord {
        reconcile(
            None,
            &created(id, ScheduleEventStatus::Scheduled, schedule),
            position(1),
            Some("event-1"),
            now(),
        )
        .unwrap()
        .next_checkpoint
    }

    #[test]
    fn enabled_creation_with_future_at_publishes_schedule() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Scheduled,
                Schedule::At {
                    at: instant("2999-01-01T00:00:00Z"),
                },
            ),
            position(1),
            Some("event-1"),
            now(),
        )
        .unwrap();

        assert!(matches!(reconciliation.action, ReconcileAction::Publish(_)));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Published);
        assert_eq!(reconciliation.next_checkpoint.last_applied_stream_position, position(1));
        assert_eq!(
            reconciliation.next_checkpoint.last_applied_event_id.as_deref(),
            Some("event-1")
        );
    }

    #[test]
    fn enabled_creation_with_at_within_the_grace_window_still_publishes() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Scheduled,
                Schedule::At {
                    at: now() - PAST_AT_GRACE + chrono::Duration::seconds(1),
                },
            ),
            position(1),
            Some("event-1"),
            now(),
        )
        .unwrap();

        assert!(matches!(reconciliation.action, ReconcileAction::Publish(_)));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Published);
    }

    #[test]
    fn enabled_creation_with_at_at_the_grace_boundary_expires() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Scheduled,
                Schedule::At {
                    at: now() - PAST_AT_GRACE,
                },
            ),
            position(1),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Expired);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Expired);
    }

    #[test]
    fn enabled_creation_with_past_at_expires_without_publishing() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Scheduled,
                Schedule::At {
                    at: instant("2000-01-01T00:00:00Z"),
                },
            ),
            position(1),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Expired);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Expired);
    }

    #[test]
    fn enabled_creation_with_rrule_publishes_the_next_one_shot_occurrence() {
        let rrule = Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap();
        let reconciliation = reconcile(
            None,
            &created("recurring", ScheduleEventStatus::Scheduled, rrule),
            position(1),
            None,
            now(),
        )
        .unwrap();

        let expected = ScheduleRequest::build_rrule_wakeup(
            &schedule_id("recurring"),
            instant("2026-06-03T00:00:00Z"),
            &Delivery::nats_event("agent.run").unwrap(),
        )
        .unwrap();
        assert_eq!(reconciliation.action, ReconcileAction::Publish(expected));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(reconciliation.next_checkpoint.last_outcome, ReconcileOutcome::Published);
    }

    #[test]
    fn rrule_occurrence_event_dispatches_and_publishes_the_next_wakeup() {
        let current = scheduled_record(
            "recurring",
            Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
        );

        let continuation = reconcile(
            Some(&current),
            &occurrence_fired("recurring", 1, "2026-06-03T00:00:00Z"),
            position(2),
            Some("event-2"),
            now(),
        )
        .unwrap();

        let expected = ScheduleRequest::build_rrule_wakeup(
            &schedule_id("recurring"),
            instant("2026-06-04T00:00:00Z"),
            &Delivery::nats_event("agent.run").unwrap(),
        )
        .unwrap();
        let expected_dispatch = DispatchRequest::build_occurrence(
            &schedule_id("recurring"),
            ScheduleOccurrenceSequence::try_new(1).unwrap(),
            instant("2026-06-03T00:00:00Z"),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();
        assert!(matches!(
            continuation.action,
            ReconcileAction::Multiple(ref actions)
                if actions.len() == 2
                    && actions[0] == ReconcileAction::Dispatch(expected_dispatch.clone())
                    && actions[1] == ReconcileAction::Publish(expected.clone())
        ));
        assert_eq!(continuation.next_checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(continuation.next_checkpoint.last_outcome, ReconcileOutcome::Published);
        assert_eq!(continuation.next_checkpoint.last_applied_stream_position, position(2));
    }

    #[test]
    fn rrule_occurrence_event_dispatches_and_expires_when_no_future_occurrence_exists() {
        let current = scheduled_record(
            "recurring",
            Schedule::rrule("2026-06-03T00:00:00Z", "FREQ=DAILY;COUNT=1", None).unwrap(),
        );

        let continuation = reconcile(
            Some(&current),
            &occurrence_fired("recurring", 1, "2026-06-03T00:00:00Z"),
            position(2),
            Some("event-2"),
            now(),
        )
        .unwrap();

        assert!(matches!(
            continuation.action,
            ReconcileAction::Multiple(ref actions)
                if actions.len() == 2
                    && matches!(actions[0], ReconcileAction::Dispatch(_))
                    && matches!(actions[1], ReconcileAction::Purge(_))
        ));
        assert_eq!(continuation.next_checkpoint.status, ScheduleStatus::Expired);
        assert_eq!(continuation.next_checkpoint.last_outcome, ReconcileOutcome::Expired);
    }

    #[test]
    fn rrule_occurrence_event_noops_for_non_rrule_checkpoints() {
        let current = scheduled_record("recurring", Schedule::every(Duration::from_secs(30)).unwrap());

        let continuation = reconcile(
            Some(&current),
            &occurrence_fired("recurring", 1, "2026-06-03T00:00:00Z"),
            position(2),
            Some("event-2"),
            now(),
        )
        .unwrap();

        assert_eq!(continuation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(
            continuation.next_checkpoint.last_outcome,
            ReconcileOutcome::DuplicateStale
        );
    }

    #[test]
    fn decide_rrule_reports_expansion_errors() {
        let error = decide_rrule(
            &Schedule::every(Duration::from_secs(30)).unwrap(),
            &schedule_id("recurring"),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
            RRuleCursor::at_or_after(now()),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ReconcileError::ScheduleRequest {
                source: ScheduleRequestError::RRuleExpansion { .. }
            }
        ));
    }

    #[test]
    fn paused_creation_stores_checkpoint_without_publishing() {
        let reconciliation = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Paused,
                Schedule::every(Duration::from_secs(30)).unwrap(),
            ),
            position(1),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Paused);
        assert_eq!(
            reconciliation.next_checkpoint.last_outcome,
            ReconcileOutcome::StoredPaused
        );
    }

    #[test]
    fn pause_purges_the_subject() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let reconciliation = reconcile(
            Some(&current),
            &ScheduleChange::Paused {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::Purge(current.subject()));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Paused);
        assert_eq!(reconciliation.next_checkpoint.last_applied_stream_position, position(2));
    }

    #[test]
    fn resume_republishes_from_the_stored_definition() {
        let paused = reconcile(
            None,
            &created(
                "orders",
                ScheduleEventStatus::Paused,
                Schedule::every(Duration::from_secs(30)).unwrap(),
            ),
            position(1),
            None,
            now(),
        )
        .unwrap()
        .next_checkpoint;

        let reconciliation = reconcile(
            Some(&paused),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap();

        assert!(matches!(reconciliation.action, ReconcileAction::Publish(_)));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Scheduled);
    }

    #[test]
    fn resume_from_removed_checkpoint_reports_unrecoverable_checkpoint() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let removed = reconcile(
            Some(&current),
            &ScheduleChange::Removed {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap()
        .next_checkpoint;

        let error = reconcile(
            Some(&removed),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(3),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::UnrecoverableCheckpoint { .. }));
    }

    #[test]
    fn resume_from_corrupt_placeholder_checkpoint_reports_unrecoverable_checkpoint() {
        let mut current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        current.status = ScheduleStatus::Paused;
        current.delivery = Delivery::nats_event(CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE).unwrap();
        current.last_outcome = ReconcileOutcome::Purged;

        let error = reconcile(
            Some(&current),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(3),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::UnrecoverableCheckpoint { .. }));
    }

    #[test]
    fn corrupt_placeholder_route_is_unclaimable_by_user_schedules() {
        assert!(ScheduleSubject::is_scheduler_internal(
            CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE
        ));
    }

    #[test]
    fn resume_rejects_a_checkpoint_for_a_different_schedule() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());

        let error = reconcile(
            Some(&current),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("invoices"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::MissingCheckpoint { .. }));
    }

    #[test]
    fn remove_purges_and_marks_removed() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let reconciliation = reconcile(
            Some(&current),
            &ScheduleChange::Removed {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap();

        assert!(matches!(reconciliation.action, ReconcileAction::Purge(_)));
        assert_eq!(reconciliation.next_checkpoint.status, ScheduleStatus::Removed);
    }

    #[test]
    fn purge_rejects_a_checkpoint_for_a_different_schedule() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());

        let error = reconcile(
            Some(&current),
            &ScheduleChange::Removed {
                schedule_id: schedule_id("invoices"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::MissingCheckpoint { .. }));
    }

    #[test]
    fn schedule_changes_without_prior_checkpoint_report_missing_checkpoint() {
        let error = reconcile(
            None,
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(2),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::MissingCheckpoint { .. }));
    }

    #[test]
    fn reconcile_errors_display_and_expose_sources() {
        let missing = ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id("orders"),
        };
        assert_eq!(
            missing.to_string(),
            "no scheduler checkpoint exists for schedule 'orders'"
        );
        assert!(std::error::Error::source(&missing).is_none());

        let unrecoverable = ReconcileError::UnrecoverableCheckpoint {
            schedule_id: schedule_id("orders"),
        };
        assert_eq!(
            unrecoverable.to_string(),
            "scheduler checkpoint for schedule 'orders' cannot be resumed"
        );
        assert!(std::error::Error::source(&unrecoverable).is_none());

        let request = ReconcileError::ScheduleRequest {
            source: ScheduleRequestError::UnsupportedSchedule,
        };
        assert_eq!(
            request.to_string(),
            "schedule request failed: schedule kind is not supported by NATS message scheduling"
        );
        assert!(std::error::Error::source(&request).is_some());
    }

    #[test]
    fn resume_with_invalid_delivery_target_fails_schedule_request() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let subject = current.subject();
        let mut current = current;
        current.delivery = Delivery::nats_event(subject.as_str()).unwrap();

        let error = reconcile(
            Some(&current),
            &ScheduleChange::Resumed {
                schedule_id: schedule_id("orders"),
            },
            position(3),
            None,
            now(),
        )
        .unwrap_err();

        assert!(matches!(error, ReconcileError::ScheduleRequest { .. }));
    }

    #[test]
    fn stale_records_are_a_no_op_that_preserves_the_definition() {
        let current = scheduled_record("orders", Schedule::every(Duration::from_secs(30)).unwrap());
        let reconciliation = reconcile(
            Some(&current),
            &ScheduleChange::Removed {
                schedule_id: schedule_id("orders"),
            },
            position(1),
            None,
            now(),
        )
        .unwrap();

        assert_eq!(reconciliation.action, ReconcileAction::CheckpointOnly);
        assert_eq!(
            reconciliation.next_checkpoint.last_outcome,
            ReconcileOutcome::DuplicateStale
        );
        assert_eq!(reconciliation.next_checkpoint.status, current.status);
        assert_eq!(reconciliation.next_checkpoint.schedule, current.schedule);
    }

    #[test]
    fn schedule_change_exposes_its_schedule_id() {
        let event = ScheduleChange::Paused {
            schedule_id: schedule_id("orders"),
        };
        assert_eq!(event.schedule_id().as_str(), "orders");
    }
}
