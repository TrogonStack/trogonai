use chrono::{DateTime, Utc};
use trogon_decider_runtime::StreamPosition;

use crate::commands::domain::{
    Delivery, Schedule, ScheduleEventStatus, ScheduleId, ScheduleMessage, ScheduleOccurrenceSequence,
};
use crate::processor::execution::checkpoints::{ReconcileOutcome, ScheduleCheckpointRecord, ScheduleStatus};

use super::{DispatchRequest, ScheduleRequest, ScheduleRequestError, ScheduleSubject};

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
    OccurrenceRecorded {
        schedule_id: ScheduleId,
        occurrence_sequence: ScheduleOccurrenceSequence,
        occurrence_at: DateTime<Utc>,
    },
    OccurrenceScheduled {
        schedule_id: ScheduleId,
        occurrence_sequence: ScheduleOccurrenceSequence,
        occurrence_at: DateTime<Utc>,
    },
    Completed {
        schedule_id: ScheduleId,
    },
}

impl ScheduleChange {
    pub fn schedule_id(&self) -> &ScheduleId {
        match self {
            Self::Created { schedule_id, .. }
            | Self::Paused { schedule_id }
            | Self::Resumed { schedule_id }
            | Self::Removed { schedule_id }
            | Self::OccurrenceRecorded { schedule_id, .. }
            | Self::OccurrenceScheduled { schedule_id, .. }
            | Self::Completed { schedule_id } => schedule_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileAction {
    Publish(ScheduleRequest),
    Dispatch(DispatchRequest),
    Purge(ScheduleSubject),
    /// Ask the schedule aggregate to plan the next occurrence. The execution
    /// processor runs the `ScheduleNextOccurrence` command; the planned occurrence
    /// returns as a `ScheduleOccurrenceScheduled` event that publishes the wakeup.
    ArmNext {
        schedule_id: ScheduleId,
        now: DateTime<Utc>,
    },
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
        ScheduleChange::OccurrenceRecorded {
            schedule_id,
            occurrence_sequence,
            occurrence_at,
        } => reconcile_occurrence_recorded(
            current,
            schedule_id,
            *occurrence_sequence,
            *occurrence_at,
            stream_position,
            event_id,
        ),
        ScheduleChange::OccurrenceScheduled {
            schedule_id,
            occurrence_sequence: _,
            occurrence_at,
        } => reconcile_occurrence_scheduled(current, schedule_id, *occurrence_at, stream_position, event_id),
        ScheduleChange::Completed { schedule_id } => {
            reconcile_completed(current, schedule_id, stream_position, event_id)
        }
    }
}

/// Dispatches the user message for a recorded occurrence.
///
/// The schedule aggregate already decided whether the recurrence continues, so
/// this only fans the occurrence out to the delivery target; the follow-up
/// `OccurrenceScheduled`/`Completed` event drives the next wakeup or purge.
fn reconcile_occurrence_recorded(
    current: Option<&ScheduleCheckpointRecord>,
    schedule_id: &ScheduleId,
    occurrence_sequence: ScheduleOccurrenceSequence,
    occurrence_at: DateTime<Utc>,
    stream_position: StreamPosition,
    event_id: Option<&str>,
) -> Result<Reconciliation, ReconcileError> {
    let current = checkpoint_for(current, schedule_id)?;
    if !matches!(current.status, ScheduleStatus::Scheduled | ScheduleStatus::Paused)
        || !matches!(current.schedule, Schedule::RRule { .. })
    {
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

    Ok(Reconciliation {
        action: ReconcileAction::Dispatch(dispatch),
        next_checkpoint: advanced(
            current,
            current.status,
            ReconcileOutcome::Published,
            stream_position,
            event_id,
        ),
    })
}

/// Publishes the concrete `@at` wakeup the aggregate planned for the next
/// occurrence. No recurrence calculation happens here: the instant comes
/// straight from the `OccurrenceScheduled` event.
fn reconcile_occurrence_scheduled(
    current: Option<&ScheduleCheckpointRecord>,
    schedule_id: &ScheduleId,
    occurrence_at: DateTime<Utc>,
    stream_position: StreamPosition,
    event_id: Option<&str>,
) -> Result<Reconciliation, ReconcileError> {
    let current = checkpoint_for(current, schedule_id)?;
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

    let request = ScheduleRequest::build_rrule_wakeup(schedule_id, occurrence_at, &current.delivery)
        .map_err(|source| ReconcileError::ScheduleRequest { source })?;

    Ok(Reconciliation {
        action: ReconcileAction::Publish(request),
        next_checkpoint: advanced(
            current,
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
            stream_position,
            event_id,
        ),
    })
}

/// Purges the execution subject once the aggregate reports the recurrence is
/// exhausted.
fn reconcile_completed(
    current: Option<&ScheduleCheckpointRecord>,
    schedule_id: &ScheduleId,
    stream_position: StreamPosition,
    event_id: Option<&str>,
) -> Result<Reconciliation, ReconcileError> {
    let current = checkpoint_for(current, schedule_id)?;
    if !matches!(current.schedule, Schedule::RRule { .. })
        || matches!(
            current.status,
            ScheduleStatus::Removed | ScheduleStatus::Unsupported | ScheduleStatus::Unknown
        )
    {
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

    Ok(Reconciliation {
        action: ReconcileAction::Purge(current.subject()),
        next_checkpoint: advanced(
            current,
            ScheduleStatus::Expired,
            ReconcileOutcome::Expired,
            stream_position,
            event_id,
        ),
    })
}

fn checkpoint_for<'a>(
    current: Option<&'a ScheduleCheckpointRecord>,
    schedule_id: &ScheduleId,
) -> Result<&'a ScheduleCheckpointRecord, ReconcileError> {
    let current = current.ok_or_else(|| ReconcileError::MissingCheckpoint {
        schedule_id: schedule_id.clone(),
    })?;
    if current.schedule_id != *schedule_id {
        return Err(ReconcileError::MissingCheckpoint {
            schedule_id: schedule_id.clone(),
        });
    }
    Ok(current)
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
    // RRULE recurrence is owned by the schedule aggregate. Hand off to the
    // arming command instead of expanding the rule here, so the execution layer
    // never performs recurrence calculation — the next concrete occurrence
    // arrives back as a `ScheduleOccurrenceScheduled` event.
    if matches!(schedule, Schedule::RRule { .. }) {
        return Ok((
            ReconcileAction::ArmNext {
                schedule_id: schedule_id.clone(),
                now,
            },
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        ));
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
mod tests;
