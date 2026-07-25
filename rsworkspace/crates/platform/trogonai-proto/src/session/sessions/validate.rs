use super::{SessionEventCase, v1alpha1};

/// Local, per-event semantic validation for a [`v1alpha1::SessionEvent`], applied at the
/// append boundary (ADR#0035 facet 3).
///
/// This checks only what a single event can prove about itself: non-empty identifiers,
/// non-unspecified required enums, required oneofs being set, and internally consistent
/// field combinations (for example a rename requiring `previous_path`).
///
/// Cross-event obligations are decide-time concerns for the aggregate, not this function,
/// and are intentionally out of scope here:
/// - id joins across a tool or execution-attempt lifecycle's phases
/// - attempt monotonicity and previous-attempt lineage
/// - the started/completed assistant message sharing an id and model
/// - a session's `StoredSessionExecutionPlan` digest matching the plan actually bound
pub fn validate_session_event(event: &v1alpha1::SessionEvent) -> Result<(), SessionEventValidationError> {
    let Some(event) = event.event.as_ref() else {
        return Err(SessionEventValidationError::MissingOneof {
            oneof: "session_event.event",
        });
    };

    match event {
        SessionEventCase::SessionStarted(inner) => validate_session_started(inner),
        SessionEventCase::SessionClosed(inner) => validate_session_closed(inner),
        SessionEventCase::SessionCancelled(inner) => validate_session_cancelled(inner),
        SessionEventCase::SessionFailed(inner) => validate_session_failed(inner),
        SessionEventCase::SessionHidden(inner) => validate_session_hidden(inner),
        SessionEventCase::SessionForked(inner) => validate_session_forked(inner),
        SessionEventCase::SessionRewound(inner) => validate_session_rewound(inner),
        SessionEventCase::Compacted(inner) => validate_compacted(inner),
        SessionEventCase::UserMessageRecorded(inner) => validate_user_message_recorded(inner),
        SessionEventCase::AssistantMessageStarted(inner) => validate_assistant_message_started(inner),
        SessionEventCase::AssistantMessageCompleted(inner) => validate_assistant_message_completed(inner),
        SessionEventCase::AssistantMessageFailed(inner) => validate_assistant_message_failed(inner),
        SessionEventCase::ToolCallRequested(inner) => validate_tool_call_requested(inner),
        SessionEventCase::ToolCallApproved(inner) => validate_tool_call_approved(inner),
        SessionEventCase::ToolCallDenied(inner) => validate_tool_call_denied(inner),
        SessionEventCase::ToolCallStarted(inner) => validate_tool_call_started(inner),
        SessionEventCase::ToolCallCompleted(inner) => validate_tool_call_completed(inner),
        SessionEventCase::ToolCallFailed(inner) => validate_tool_call_failed(inner),
        SessionEventCase::ArtifactRecorded(inner) => validate_artifact_recorded(inner),
        SessionEventCase::FileChanged(inner) => validate_file_changed(inner),
        SessionEventCase::ExecutionAttemptStarted(inner) => validate_execution_attempt_started(inner),
        SessionEventCase::ExecutionAttemptReady(inner) => validate_execution_attempt_ready(inner),
        SessionEventCase::ExecutionAttemptEnded(inner) => validate_execution_attempt_ended(inner),
        SessionEventCase::CheckpointProduced(inner) => validate_checkpoint_produced(inner),
        SessionEventCase::DelegationDispatched(inner) => validate_delegation_dispatched(inner),
        SessionEventCase::ParentLinked(inner) => validate_parent_linked(inner),
        SessionEventCase::ParentTerminated(inner) => validate_parent_terminated(inner),
        SessionEventCase::DelegationDetached(inner) => validate_delegation_detached(inner),
        SessionEventCase::ParentHistoryInvalidated(inner) => validate_parent_history_invalidated(inner),
        SessionEventCase::ParentDetached(inner) => validate_parent_detached(inner),
        SessionEventCase::ExternalDelegationDispatched(inner) => validate_external_delegation_dispatched(inner),
        SessionEventCase::OperationReserved(inner) => validate_operation_reserved(inner),
        SessionEventCase::OperationOutcomeRecorded(inner) => validate_operation_outcome_recorded(inner),
        SessionEventCase::OperationCancellationRequested(inner) => validate_operation_cancellation_requested(inner),
        SessionEventCase::ArtifactErased(inner) => validate_artifact_erased(inner),
        SessionEventCase::RedactionApplied(inner) => validate_redaction_applied(inner),
        SessionEventCase::SystemNoticeRecorded(inner) => validate_system_notice_recorded(inner),
        SessionEventCase::TodoUpdated(inner) => validate_todo_updated(inner),
        SessionEventCase::SessionRenamed(inner) => validate_session_renamed(inner),
        SessionEventCase::SessionArchived(inner) => validate_session_archived(inner),
        SessionEventCase::SessionUnarchived(inner) => validate_session_unarchived(inner),
    }
}

/// Failure from [`validate_session_event`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionEventValidationError {
    #[error("{field} must not be empty")]
    EmptyIdentifier { field: &'static str },

    #[error("{field} must be a known, non-unspecified enum value")]
    UnspecifiedEnum { field: &'static str },

    #[error("{oneof} must be set")]
    MissingOneof { oneof: &'static str },

    #[error("{field} must be {expected}")]
    UnexpectedMessageRole {
        field: &'static str,
        expected: &'static str,
    },

    #[error("previous_path must be set when change_kind is FILE_CHANGE_KIND_RENAMED")]
    RenamedFileChangeMissingPreviousPath,

    #[error("previous_path must be unset unless change_kind is FILE_CHANGE_KIND_RENAMED")]
    NonRenamedFileChangeHasPreviousPath,

    #[error("covers_through.value ({covers_through}) must be >= covers_from.value ({covers_from})")]
    CompactionRangeOutOfOrder { covers_from: u64, covers_through: u64 },

    #[error("{field}.value must be >= 1")]
    OrdinalNotPositive { field: &'static str },

    #[error("matched_stop_sequence must be set when finish_reason is FINISH_REASON_STOP_SEQUENCE")]
    MissingMatchedStopSequence,

    #[error("matched_stop_sequence must be unset unless finish_reason is FINISH_REASON_STOP_SEQUENCE")]
    UnexpectedMatchedStopSequence,

    #[error("attempt_number must be >= 1")]
    AttemptNumberNotPositive,

    #[error("todo item id must not be empty")]
    EmptyTodoItemId,

    #[error("duplicate todo item id '{id}'")]
    DuplicateTodoItemId { id: String },

    #[error("revision must be >= 1")]
    TodoRevisionNotPositive,

    #[error("redacted_event_ids must not be empty")]
    EmptyRedactedEventIds,

    #[error("{field}.algorithm must not be empty")]
    EmptyDigestAlgorithm { field: &'static str },

    #[error("{field}.value must be exactly 32 bytes for algorithm sha256, got {actual}")]
    Sha256DigestWrongLength { field: &'static str, actual: usize },
}

fn require_non_empty(value: &str, field: &'static str) -> Result<(), SessionEventValidationError> {
    if value.is_empty() {
        Err(SessionEventValidationError::EmptyIdentifier { field })
    } else {
        Ok(())
    }
}

fn require_known_nonzero<E>(value: buffa::EnumValue<E>, field: &'static str) -> Result<(), SessionEventValidationError>
where
    E: buffa::Enumeration,
{
    match value.as_known() {
        Some(known) if known.to_i32() != 0 => Ok(()),
        _ => Err(SessionEventValidationError::UnspecifiedEnum { field }),
    }
}

fn require_positive_ordinal(
    ordinal: &v1alpha1::SessionOrdinal,
    field: &'static str,
) -> Result<(), SessionEventValidationError> {
    if ordinal.value == 0 {
        Err(SessionEventValidationError::OrdinalNotPositive { field })
    } else {
        Ok(())
    }
}

fn require_digest(digest: &v1alpha1::Digest, field: &'static str) -> Result<(), SessionEventValidationError> {
    if digest.algorithm.is_empty() {
        return Err(SessionEventValidationError::EmptyDigestAlgorithm { field });
    }
    if digest.algorithm == "sha256" && digest.value.len() != 32 {
        return Err(SessionEventValidationError::Sha256DigestWrongLength {
            field,
            actual: digest.value.len(),
        });
    }
    Ok(())
}

fn validate_canonical_message(
    message: &v1alpha1::CanonicalMessage,
    field: &'static str,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&message.message_id, field)?;
    for block in &message.content {
        if block.kind.is_none() {
            return Err(SessionEventValidationError::MissingOneof {
                oneof: "content_block.kind",
            });
        }
    }
    Ok(())
}

fn validate_checkpoint(checkpoint: &v1alpha1::Checkpoint) -> Result<(), SessionEventValidationError> {
    require_non_empty(&checkpoint.checkpoint_id, "checkpoint.checkpoint_id")?;
    require_non_empty(&checkpoint.reference, "checkpoint.reference")?;
    require_non_empty(
        &checkpoint.producing_execution_attempt_id,
        "checkpoint.producing_execution_attempt_id",
    )?;
    require_positive_ordinal(&checkpoint.covers_through, "checkpoint.covers_through")?;
    require_digest(&checkpoint.digest, "checkpoint.digest")?;
    require_digest(
        &checkpoint.session_execution_plan_digest,
        "checkpoint.session_execution_plan_digest",
    )?;
    Ok(())
}

fn validate_session_started(event: &v1alpha1::SessionStarted) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_digest(&event.execution_plan.plan_digest, "execution_plan.plan_digest")?;
    Ok(())
}

fn validate_session_closed(event: &v1alpha1::SessionClosed) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")
}

fn validate_session_cancelled(event: &v1alpha1::SessionCancelled) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_session_failed(event: &v1alpha1::SessionFailed) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_session_hidden(event: &v1alpha1::SessionHidden) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_session_forked(event: &v1alpha1::SessionForked) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.source_session_id, "source_session_id")?;
    require_positive_ordinal(&event.context_prefix_boundary, "context_prefix_boundary")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_session_rewound(event: &v1alpha1::SessionRewound) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_positive_ordinal(&event.keep_through, "keep_through")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_compacted(event: &v1alpha1::Compacted) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.summary_id, "summary_id")?;
    require_positive_ordinal(&event.covers_from, "covers_from")?;
    require_positive_ordinal(&event.covers_through, "covers_through")?;
    if event.covers_through.value < event.covers_from.value {
        return Err(SessionEventValidationError::CompactionRangeOutOfOrder {
            covers_from: event.covers_from.value,
            covers_through: event.covers_through.value,
        });
    }
    require_known_nonzero(event.trigger, "trigger")
}

fn validate_user_message_recorded(event: &v1alpha1::UserMessageRecorded) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    validate_canonical_message(&event.message, "message.message_id")?;
    if event.message.role != v1alpha1::MessageRole::User {
        return Err(SessionEventValidationError::UnexpectedMessageRole {
            field: "message.role",
            expected: "MESSAGE_ROLE_USER",
        });
    }
    Ok(())
}

fn validate_assistant_message_started(
    event: &v1alpha1::AssistantMessageStarted,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.message_id, "message_id")
}

fn validate_assistant_message_completed(
    event: &v1alpha1::AssistantMessageCompleted,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    validate_canonical_message(&event.message, "message.message_id")?;
    if event.message.role != v1alpha1::MessageRole::Assistant {
        return Err(SessionEventValidationError::UnexpectedMessageRole {
            field: "message.role",
            expected: "MESSAGE_ROLE_ASSISTANT",
        });
    }
    require_known_nonzero(event.finish_reason, "finish_reason")?;

    let is_stop_sequence = event.finish_reason == v1alpha1::FinishReason::StopSequence;
    let has_matched_stop_sequence = event.matched_stop_sequence.as_deref().is_some_and(|s| !s.is_empty());
    match (is_stop_sequence, has_matched_stop_sequence) {
        (true, false) => Err(SessionEventValidationError::MissingMatchedStopSequence),
        (false, true) => Err(SessionEventValidationError::UnexpectedMatchedStopSequence),
        _ => Ok(()),
    }
}

fn validate_assistant_message_failed(
    event: &v1alpha1::AssistantMessageFailed,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.message_id, "message_id")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_tool_call_requested(event: &v1alpha1::ToolCallRequested) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.name, "name")
}

fn validate_tool_call_approved(event: &v1alpha1::ToolCallApproved) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.approved_by, "approved_by")
}

fn validate_tool_call_denied(event: &v1alpha1::ToolCallDenied) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.denied_by, "denied_by")
}

fn validate_tool_call_started(event: &v1alpha1::ToolCallStarted) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")
}

fn validate_tool_call_completed(event: &v1alpha1::ToolCallCompleted) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_known_nonzero(event.result.status, "result.status")?;
    if event.result.kind.is_none() {
        return Err(SessionEventValidationError::MissingOneof {
            oneof: "tool_call_result.kind",
        });
    }
    Ok(())
}

fn validate_tool_call_failed(event: &v1alpha1::ToolCallFailed) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_artifact_recorded(event: &v1alpha1::ArtifactRecorded) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.artifact.artifact_id, "artifact.artifact_id")?;
    if event.artifact.source.is_none() {
        return Err(SessionEventValidationError::MissingOneof {
            oneof: "artifact_metadata.source",
        });
    }
    Ok(())
}

fn validate_file_changed(event: &v1alpha1::FileChanged) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.path, "path")?;
    require_known_nonzero(event.change_kind, "change_kind")?;

    let is_renamed = event.change_kind == v1alpha1::FileChangeKind::Renamed;
    let has_previous_path = event.previous_path.as_deref().is_some_and(|s| !s.is_empty());
    match (is_renamed, has_previous_path) {
        (true, false) => Err(SessionEventValidationError::RenamedFileChangeMissingPreviousPath),
        (false, true) => Err(SessionEventValidationError::NonRenamedFileChangeHasPreviousPath),
        _ => Ok(()),
    }
}

fn validate_execution_attempt_started(
    event: &v1alpha1::ExecutionAttemptStarted,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.execution_attempt_id, "execution_attempt_id")?;
    require_non_empty(&event.host_artifact_ref, "host_artifact_ref")?;
    require_digest(&event.session_execution_plan_digest, "session_execution_plan_digest")?;
    require_digest(&event.host_artifact_digest, "host_artifact_digest")?;
    if event.attempt_number < 1 {
        return Err(SessionEventValidationError::AttemptNumberNotPositive);
    }
    if let Some(checkpoint) = event.restored_checkpoint.as_option() {
        validate_checkpoint(checkpoint)?;
    }
    Ok(())
}

fn validate_execution_attempt_ready(
    event: &v1alpha1::ExecutionAttemptReady,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.execution_attempt_id, "execution_attempt_id")?;
    require_non_empty(&event.ready_attestation_ref, "ready_attestation_ref")?;
    require_digest(&event.ready_attestation_digest, "ready_attestation_digest")
}

fn validate_execution_attempt_ended(
    event: &v1alpha1::ExecutionAttemptEnded,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.execution_attempt_id, "execution_attempt_id")?;
    require_known_nonzero(event.outcome, "outcome")
}

fn validate_checkpoint_produced(event: &v1alpha1::CheckpointProduced) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    validate_checkpoint(&event.checkpoint)
}

fn validate_delegation_dispatched(event: &v1alpha1::DelegationDispatched) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.operation_id, "operation_id")?;
    require_non_empty(&event.child_session_id, "child_session_id")?;
    require_known_nonzero(event.cascade_policy, "cascade_policy")
}

fn validate_parent_linked(event: &v1alpha1::ParentLinked) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.parent_session_id, "parent_session_id")?;
    require_non_empty(&event.operation_id, "operation_id")?;
    require_positive_ordinal(&event.parent_dispatched_at, "parent_dispatched_at")?;
    require_known_nonzero(event.cascade_policy, "cascade_policy")
}

fn validate_parent_terminated(event: &v1alpha1::ParentTerminated) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.parent_session_id, "parent_session_id")?;
    require_non_empty(&event.triggering_event_id, "triggering_event_id")?;
    require_known_nonzero(event.cause, "cause")
}

fn validate_delegation_detached(event: &v1alpha1::DelegationDetached) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.child_session_id, "child_session_id")?;
    require_non_empty(&event.detach_operation_id, "detach_operation_id")
}

fn validate_parent_history_invalidated(
    event: &v1alpha1::ParentHistoryInvalidated,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.parent_session_id, "parent_session_id")?;
    require_non_empty(&event.triggering_event_id, "triggering_event_id")?;
    require_positive_ordinal(&event.parent_keep_through, "parent_keep_through")
}

fn validate_parent_detached(event: &v1alpha1::ParentDetached) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.parent_session_id, "parent_session_id")?;
    require_non_empty(&event.detach_operation_id, "detach_operation_id")
}

fn validate_external_delegation_dispatched(
    event: &v1alpha1::ExternalDelegationDispatched,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.operation_id, "operation_id")?;
    require_non_empty(&event.delegate_reference, "delegate_reference")?;
    require_non_empty(&event.authenticated_remote_subject, "authenticated_remote_subject")?;
    require_non_empty(&event.authorization_reference, "authorization_reference")?;
    require_non_empty(&event.correlation_id, "correlation_id")?;
    require_digest(&event.request_digest, "request_digest")
}

fn validate_operation_reserved(event: &v1alpha1::OperationReserved) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.operation_id, "operation_id")?;
    require_known_nonzero(event.operation_kind, "operation_kind")?;
    require_digest(&event.request_digest, "request_digest")
}

fn validate_operation_outcome_recorded(
    event: &v1alpha1::OperationOutcomeRecorded,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.operation_id, "operation_id")?;
    if event.outcome.is_none() {
        return Err(SessionEventValidationError::MissingOneof {
            oneof: "operation_outcome_recorded.outcome",
        });
    }
    Ok(())
}

fn validate_operation_cancellation_requested(
    event: &v1alpha1::OperationCancellationRequested,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.operation_id, "operation_id")
}

fn validate_artifact_erased(event: &v1alpha1::ArtifactErased) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.artifact_id, "artifact_id")
}

fn validate_redaction_applied(event: &v1alpha1::RedactionApplied) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    if event.redacted_event_ids.is_empty() {
        return Err(SessionEventValidationError::EmptyRedactedEventIds);
    }
    for (index, id) in event.redacted_event_ids.iter().enumerate() {
        if id.is_empty() {
            return Err(SessionEventValidationError::EmptyIdentifier {
                field: if index == 0 {
                    "redacted_event_ids[0]"
                } else {
                    "redacted_event_ids[n]"
                },
            });
        }
    }
    Ok(())
}

fn validate_system_notice_recorded(event: &v1alpha1::SystemNoticeRecorded) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_known_nonzero(event.level, "level")
}

fn validate_todo_updated(event: &v1alpha1::TodoUpdated) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    if event.revision < 1 {
        return Err(SessionEventValidationError::TodoRevisionNotPositive);
    }

    let mut seen_ids = std::collections::HashSet::with_capacity(event.items.len());
    for item in &event.items {
        if item.id.is_empty() {
            return Err(SessionEventValidationError::EmptyTodoItemId);
        }
        if !seen_ids.insert(item.id.as_str()) {
            return Err(SessionEventValidationError::DuplicateTodoItemId { id: item.id.clone() });
        }
        require_known_nonzero(item.status, "items[].status")?;
    }
    Ok(())
}

fn validate_session_renamed(event: &v1alpha1::SessionRenamed) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")
}

fn validate_session_archived(event: &v1alpha1::SessionArchived) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")
}

fn validate_session_unarchived(event: &v1alpha1::SessionUnarchived) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")
}

#[cfg(test)]
mod tests;
