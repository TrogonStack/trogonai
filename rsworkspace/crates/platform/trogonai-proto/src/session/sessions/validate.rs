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
/// - an identifier matching the addressed stream, which requires the stream's own identity
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
        SessionEventCase::SessionRecovered(inner) => validate_session_recovered(inner),
        SessionEventCase::SessionRewound(inner) => validate_session_rewound(inner),
        SessionEventCase::Compacted(inner) => validate_compacted(inner),
        SessionEventCase::UserMessageRecorded(inner) => validate_user_message_recorded(inner),
        SessionEventCase::AssistantMessageStarted(inner) => validate_assistant_message_started(inner),
        SessionEventCase::AssistantMessageCompleted(inner) => validate_assistant_message_completed(inner),
        SessionEventCase::AssistantMessageFailed(inner) => validate_assistant_message_failed(inner),
        SessionEventCase::ToolCallRequested(inner) => validate_tool_call_requested(inner),
        SessionEventCase::ToolCallApproved(inner) => validate_tool_call_approved(inner),
        SessionEventCase::ProviderToolIntentRejected(inner) => validate_provider_tool_intent_rejected(inner),
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

    #[error("{field} must be set")]
    MissingRequiredField { field: &'static str },

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

    #[error("covers_from.value ({covers_from}) must be 1; a compaction covers the whole own-stream prefix")]
    CompactionCoversFromNotOwnStreamStart { covers_from: u64 },

    #[error("{field}.value must be >= 1")]
    OrdinalNotPositive { field: &'static str },

    #[error("matched_stop_sequence must be set when finish_reason is FINISH_REASON_STOP_SEQUENCE")]
    MissingMatchedStopSequence,

    #[error("matched_stop_sequence must be unset unless finish_reason is FINISH_REASON_STOP_SEQUENCE")]
    UnexpectedMatchedStopSequence,

    #[error("attempt_number must be >= 1")]
    AttemptNumberNotPositive,

    #[error("previous_attempt_id must be unset when attempt_number is 1")]
    FirstAttemptHasPreviousAttemptId,

    #[error("previous_attempt_id must be set when attempt_number is greater than 1")]
    RestartAttemptMissingPreviousAttemptId,

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

    #[error("{field}.algorithm must be \"sha256\"; v1alpha1 supports no other digest algorithm (ADR#0035 facet 3)")]
    UnsupportedDigestAlgorithm { field: &'static str },

    #[error("{field}.value must be exactly 32 bytes for algorithm sha256, got {actual}")]
    Sha256DigestWrongLength { field: &'static str, actual: usize },

    #[error("restored_checkpoint.session_execution_plan_digest must match session_execution_plan_digest")]
    RestoredCheckpointPlanDigestMismatch,

    #[error("omitted_count must be 0 when completeness is COMPLETE, got {actual}")]
    CompleteRecoveryWithOmissions { actual: u32 },

    #[error("omitted_count must be >= 1 when completeness is PARTIAL")]
    PartialRecoveryWithoutOmissions,

    #[error("{field} must be well-formed JSON")]
    InvalidJson { field: &'static str },

    #[error("{field} must be set")]
    MissingTimestamp { field: &'static str },

    #[error("{field} must be a valid timestamp")]
    InvalidTimestamp { field: &'static str },

    #[error("{field} must be a valid ISO 4217 currency code")]
    InvalidCurrencyCode { field: &'static str },

    #[error("{field} must be a non-negative duration with nanos below one second")]
    InvalidDuration { field: &'static str },

    #[error("{field} must be a finite number")]
    NonFiniteSetting { field: &'static str },

    #[error("untruncated_size_bytes ({untruncated}) must be greater than size_bytes ({size})")]
    UntruncatedSizeNotGreater { size: u64, untruncated: u64 },

    #[error("diff.rendered must be set when diff.truncated is true")]
    TruncatedDiffWithoutRender,

    #[error("{field}.length must be >= 1")]
    EmptyByteRange { field: &'static str },

    #[error("observed[].range must be unset when the resource was absent")]
    RangeWithAbsentObservation,
}

fn require_non_empty(value: &str, field: &'static str) -> Result<(), SessionEventValidationError> {
    if value.is_empty() {
        Err(SessionEventValidationError::EmptyIdentifier { field })
    } else {
        Ok(())
    }
}

fn require_non_empty_when_set(value: Option<&str>, field: &'static str) -> Result<(), SessionEventValidationError> {
    match value {
        Some(value) => require_non_empty(value, field),
        None => Ok(()),
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
    if digest.algorithm != "sha256" {
        return Err(SessionEventValidationError::UnsupportedDigestAlgorithm { field });
    }
    if digest.value.len() != 32 {
        return Err(SessionEventValidationError::Sha256DigestWrongLength {
            field,
            actual: digest.value.len(),
        });
    }
    Ok(())
}

fn require_valid_json(value: &str, field: &'static str) -> Result<(), SessionEventValidationError> {
    serde_json::from_str::<serde_json::Value>(value)
        .map(|_| ())
        .map_err(|_| SessionEventValidationError::InvalidJson { field })
}

fn require_valid_timestamp(
    timestamp: &buffa_types::google::protobuf::Timestamp,
    field: &'static str,
) -> Result<(), SessionEventValidationError> {
    crate::convert::datetime_from_timestamp(timestamp)
        .map(|_| ())
        .map_err(|_| SessionEventValidationError::InvalidTimestamp { field })
}

fn require_set_timestamp<P: buffa::ProtoBox<buffa_types::google::protobuf::Timestamp>>(
    timestamp: &buffa::MessageField<buffa_types::google::protobuf::Timestamp, P>,
    field: &'static str,
) -> Result<(), SessionEventValidationError> {
    match timestamp.as_option() {
        Some(value) => require_valid_timestamp(value, field),
        None => Err(SessionEventValidationError::MissingTimestamp { field }),
    }
}

fn require_valid_duration(
    duration: &buffa_types::google::protobuf::Duration,
    field: &'static str,
) -> Result<(), SessionEventValidationError> {
    crate::convert::std_from_duration(duration)
        .map(|_| ())
        .map_err(|_| SessionEventValidationError::InvalidDuration { field })
}

fn require_finite(value: f64, field: &'static str) -> Result<(), SessionEventValidationError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(SessionEventValidationError::NonFiniteSetting { field })
    }
}

fn require_iso4217_currency_code(code: &str, field: &'static str) -> Result<(), SessionEventValidationError> {
    if code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err(SessionEventValidationError::InvalidCurrencyCode { field })
    }
}

fn validate_token_usage(
    usage: &v1alpha1::TokenUsage,
    currency_code_field: &'static str,
) -> Result<(), SessionEventValidationError> {
    if let Some(cost) = usage.cost.as_option() {
        require_iso4217_currency_code(&cost.currency_code, currency_code_field)?;
    }
    if let Some(completeness) = usage.completeness {
        require_known_nonzero(completeness, "usage.completeness")?;
    }
    Ok(())
}

fn validate_model_settings(settings: &v1alpha1::ModelSettings) -> Result<(), SessionEventValidationError> {
    if let Some(temperature) = settings.temperature {
        require_finite(temperature, "settings.temperature")?;
    }
    if let Some(top_p) = settings.top_p {
        require_finite(top_p, "settings.top_p")?;
    }
    for stop_sequence in &settings.stop_sequences {
        require_non_empty(stop_sequence, "settings.stop_sequences[]")?;
    }
    if let Some(raw_settings) = settings.raw_settings.as_option() {
        validate_artifact_ref(raw_settings)?;
    }
    Ok(())
}

fn validate_workspace_ref(workspace: &v1alpha1::WorkspaceRef) -> Result<(), SessionEventValidationError> {
    require_non_empty(&workspace.workspace_id, "workspace.workspace_id")?;
    require_non_empty(&workspace.uri, "workspace.uri")
}

fn validate_diff_summary(diff: &v1alpha1::DiffSummary) -> Result<(), SessionEventValidationError> {
    let rendered = diff.rendered.as_option();
    if diff.truncated == Some(true) && rendered.is_none() {
        return Err(SessionEventValidationError::TruncatedDiffWithoutRender);
    }
    if let Some(rendered) = rendered {
        validate_artifact_ref(rendered)?;
    }
    Ok(())
}

fn validate_resource_observation(
    observation: &v1alpha1::ResourceObservation,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&observation.uri, "observed[].uri")?;
    let range = observation.range.as_option();
    match observation.outcome.as_ref() {
        Some(v1alpha1::resource_observation::Outcome::ContentDigest(content_digest)) => {
            require_digest(content_digest, "observed[].content_digest")?;
        }
        Some(v1alpha1::resource_observation::Outcome::Absent(_)) => {
            if range.is_some() {
                return Err(SessionEventValidationError::RangeWithAbsentObservation);
            }
        }
        None => {
            return Err(SessionEventValidationError::MissingOneof {
                oneof: "observed[].outcome",
            });
        }
    }
    if let Some(range) = range
        && range.length == 0
    {
        return Err(SessionEventValidationError::EmptyByteRange {
            field: "observed[].range",
        });
    }
    Ok(())
}

fn validate_tool_call_result(result: &v1alpha1::ToolCallResult) -> Result<(), SessionEventValidationError> {
    require_known_nonzero(result.status, "result.status")?;
    let Some(kind) = result.kind.as_ref() else {
        return Err(SessionEventValidationError::MissingOneof {
            oneof: "tool_call_result.kind",
        });
    };
    match kind {
        v1alpha1::tool_call_result::Kind::Text(text) => {
            require_non_empty(&text.content, "tool_call_result.text.content")
        }
        v1alpha1::tool_call_result::Kind::ArtifactRef(artifact_ref) => validate_artifact_ref(artifact_ref),
    }
}

fn validate_canonical_message(
    message: &v1alpha1::CanonicalMessage,
    field: &'static str,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&message.message_id, field)?;
    for block in &message.content {
        let Some(kind) = block.kind.as_ref() else {
            return Err(SessionEventValidationError::MissingOneof {
                oneof: "content_block.kind",
            });
        };
        match kind {
            v1alpha1::content_block::Kind::Text(_) => {}
            v1alpha1::content_block::Kind::ArtifactRef(artifact_ref) => {
                validate_artifact_ref(artifact_ref)?;
            }
            v1alpha1::content_block::Kind::Thinking(thinking) => {
                require_non_empty(&thinking.text, "content_block.thinking.text")?;
            }
            v1alpha1::content_block::Kind::ToolUse(tool_use) => {
                require_non_empty(&tool_use.id, "content_block.tool_use.id")?;
                require_non_empty(&tool_use.name, "content_block.tool_use.name")?;
                require_valid_json(&tool_use.input_json, "content_block.tool_use.input_json")?;
            }
            v1alpha1::content_block::Kind::ToolResult(tool_result) => {
                require_non_empty(&tool_result.tool_use_id, "content_block.tool_result.tool_use_id")?;
                validate_tool_call_result(&tool_result.result)?;
            }
            v1alpha1::content_block::Kind::RedactedThinking(_) => {}
            v1alpha1::content_block::Kind::Provider(provider) => {
                require_non_empty(&provider.provider, "content_block.provider.provider")?;
                require_non_empty(&provider.block_type, "content_block.provider.block_type")?;
                let Some(payload) = provider.payload.as_ref() else {
                    return Err(SessionEventValidationError::MissingOneof {
                        oneof: "provider_block.payload",
                    });
                };
                match payload {
                    v1alpha1::provider_block::Payload::Inline(inline) if inline.is_empty() => {
                        return Err(SessionEventValidationError::EmptyIdentifier {
                            field: "content_block.provider.inline",
                        });
                    }
                    v1alpha1::provider_block::Payload::Inline(_) => {}
                    v1alpha1::provider_block::Payload::Ref(artifact_ref) => {
                        validate_artifact_ref(artifact_ref)?;
                    }
                }
            }
        }
    }
    if let Some(usage) = message.usage.as_option() {
        validate_token_usage(usage, "message.usage.cost.currency_code")?;
    }
    require_set_timestamp(&message.created_at, "message.created_at")?;
    Ok(())
}

fn validate_checkpoint(checkpoint: &v1alpha1::Checkpoint) -> Result<(), SessionEventValidationError> {
    require_non_empty(&checkpoint.checkpoint_id, "checkpoint.checkpoint_id")?;
    require_non_empty(&checkpoint.reference, "checkpoint.reference")?;
    require_non_empty(&checkpoint.checkpoint_type, "checkpoint.checkpoint_type")?;
    require_non_empty(&checkpoint.implementation_version, "checkpoint.implementation_version")?;
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
    require_non_empty(
        &checkpoint.capture_attestation_ref,
        "checkpoint.capture_attestation_ref",
    )?;
    require_digest(
        &checkpoint.capture_attestation_digest,
        "checkpoint.capture_attestation_digest",
    )?;
    require_digest(
        &checkpoint.effective_history_digest,
        "checkpoint.effective_history_digest",
    )?;
    Ok(())
}

fn validate_artifact_ref(artifact_ref: &v1alpha1::ArtifactRef) -> Result<(), SessionEventValidationError> {
    require_non_empty(&artifact_ref.artifact_id, "artifact_ref.artifact_id")?;
    require_digest(&artifact_ref.digest, "artifact_ref.digest")?;
    require_non_empty(&artifact_ref.mime, "artifact_ref.mime")?;
    if let Some(untruncated) = artifact_ref.untruncated_size_bytes
        && untruncated <= artifact_ref.size_bytes
    {
        return Err(SessionEventValidationError::UntruncatedSizeNotGreater {
            size: artifact_ref.size_bytes,
            untruncated,
        });
    }
    Ok(())
}

fn validate_artifact_metadata_source(
    source: &v1alpha1::artifact_metadata::Source,
) -> Result<(), SessionEventValidationError> {
    match source {
        v1alpha1::artifact_metadata::Source::Stored(stored) => {
            require_digest(&stored.digest, "artifact_metadata.stored.digest")?;
            require_non_empty(&stored.storage_ref, "artifact_metadata.stored.storage_ref")?;
            require_non_empty(&stored.mime, "artifact_metadata.stored.mime")
        }
        v1alpha1::artifact_metadata::Source::External(external) => {
            require_non_empty(&external.source_url, "artifact_metadata.external.source_url")?;
            if let Some(content_digest) = external.content_digest.as_option() {
                require_digest(content_digest, "artifact_metadata.external.content_digest")?;
            }
            if let Some(fetched_at) = external.fetched_at.as_option() {
                require_valid_timestamp(fetched_at, "artifact_metadata.external.fetched_at")?;
            }
            Ok(())
        }
    }
}

fn validate_operation_outcome(
    outcome: &v1alpha1::operation_outcome_recorded::Outcome,
) -> Result<(), SessionEventValidationError> {
    match outcome {
        v1alpha1::operation_outcome_recorded::Outcome::Succeeded(succeeded) => {
            require_digest(
                &succeeded.response_digest,
                "operation_outcome_recorded.succeeded.response_digest",
            )?;
            if let Some(response_ref) = succeeded.response_ref.as_option() {
                validate_artifact_ref(response_ref)?;
            }
            Ok(())
        }
        v1alpha1::operation_outcome_recorded::Outcome::Failed(failed) => {
            require_non_empty(&failed.detail, "operation_outcome_recorded.failed.detail")?;
            if let Some(failure_digest) = failed.failure_digest.as_option() {
                require_digest(failure_digest, "operation_outcome_recorded.failed.failure_digest")?;
            }
            Ok(())
        }
        v1alpha1::operation_outcome_recorded::Outcome::Cancelled(cancelled) => require_non_empty(
            &cancelled.cancelled_by,
            "operation_outcome_recorded.cancelled.cancelled_by",
        ),
        v1alpha1::operation_outcome_recorded::Outcome::Unknown(_) => Ok(()),
    }
}

fn validate_session_started(event: &v1alpha1::SessionStarted) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    if event.execution_plan.plan_bytes.is_empty() {
        return Err(SessionEventValidationError::EmptyIdentifier {
            field: "execution_plan.plan_bytes",
        });
    }
    require_digest(&event.execution_plan.plan_digest, "execution_plan.plan_digest")?;
    validate_workspace_ref(&event.workspace)
}

fn validate_session_closed(event: &v1alpha1::SessionClosed) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    if let Some(result_ref) = event.result_ref.as_option() {
        validate_artifact_ref(result_ref)?;
    }
    Ok(())
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

fn validate_session_recovered(event: &v1alpha1::SessionRecovered) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.source_session_id, "source_session_id")?;
    require_positive_ordinal(&event.source_boundary, "source_boundary")?;
    let source_digest = event
        .source_digest
        .as_option()
        .ok_or(SessionEventValidationError::MissingRequiredField { field: "source_digest" })?;
    require_digest(source_digest, "source_digest")?;
    require_non_empty(&event.salvage_key, "salvage_key")?;
    require_known_nonzero(event.completeness, "completeness")?;
    // A complete recovery that lost something, or a partial one that lost
    // nothing, would let a reader draw the opposite conclusion from each field.
    match event.completeness.as_known() {
        Some(v1alpha1::RecoveryCompleteness::Complete) if event.omitted_count != 0 => {
            Err(SessionEventValidationError::CompleteRecoveryWithOmissions {
                actual: event.omitted_count,
            })
        }
        Some(v1alpha1::RecoveryCompleteness::Partial) if event.omitted_count == 0 => {
            Err(SessionEventValidationError::PartialRecoveryWithoutOmissions)
        }
        _ => Ok(()),
    }
}

fn validate_session_rewound(event: &v1alpha1::SessionRewound) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_positive_ordinal(&event.keep_through, "keep_through")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_compacted(event: &v1alpha1::Compacted) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.summary_id, "summary_id")?;
    require_non_empty(&event.summary_content, "summary_content")?;
    require_positive_ordinal(&event.covers_from, "covers_from")?;
    require_positive_ordinal(&event.covers_through, "covers_through")?;
    if event.covers_through.value < event.covers_from.value {
        return Err(SessionEventValidationError::CompactionRangeOutOfOrder {
            covers_from: event.covers_from.value,
            covers_through: event.covers_through.value,
        });
    }
    if event.covers_from.value != 1 {
        return Err(SessionEventValidationError::CompactionCoversFromNotOwnStreamStart {
            covers_from: event.covers_from.value,
        });
    }
    require_known_nonzero(event.trigger, "trigger")?;
    require_non_empty_when_set(event.model.as_deref(), "model")?;
    if let Some(usage) = event.usage.as_option() {
        validate_token_usage(usage, "usage.cost.currency_code")?;
    }
    let context_root = event
        .context_root
        .as_option()
        .ok_or(SessionEventValidationError::MissingRequiredField { field: "context_root" })?;
    validate_compaction_context_root(context_root)?;
    let producer = event
        .producer
        .as_option()
        .ok_or(SessionEventValidationError::MissingRequiredField { field: "producer" })?;
    validate_compaction_producer(producer)?;
    let covered_input_digest =
        event
            .covered_input_digest
            .as_option()
            .ok_or(SessionEventValidationError::MissingRequiredField {
                field: "covered_input_digest",
            })?;
    require_digest(covered_input_digest, "covered_input_digest")?;
    Ok(())
}

fn validate_compaction_context_root(
    context_root: &v1alpha1::CompactionContextRoot,
) -> Result<(), SessionEventValidationError> {
    match context_root.root.as_ref() {
        Some(v1alpha1::compaction_context_root::Root::SessionStart(_)) => Ok(()),
        Some(v1alpha1::compaction_context_root::Root::InheritedPrefix(inherited_prefix)) => {
            require_non_empty(
                &inherited_prefix.source_session_id,
                "context_root.inherited_prefix.source_session_id",
            )?;
            require_positive_ordinal(
                &inherited_prefix.context_prefix_boundary,
                "context_root.inherited_prefix.context_prefix_boundary",
            )
        }
        None => Err(SessionEventValidationError::MissingOneof {
            oneof: "compaction_context_root.root",
        }),
    }
}

fn validate_compaction_producer(producer: &v1alpha1::CompactionProducer) -> Result<(), SessionEventValidationError> {
    require_non_empty(
        &producer.producing_execution_attempt_id,
        "producer.producing_execution_attempt_id",
    )?;
    let session_execution_plan_digest = producer.session_execution_plan_digest.as_option().ok_or(
        SessionEventValidationError::MissingRequiredField {
            field: "producer.session_execution_plan_digest",
        },
    )?;
    require_digest(session_execution_plan_digest, "producer.session_execution_plan_digest")?;
    require_known_nonzero(producer.model_role, "producer.model_role")
}

fn validate_user_message_recorded(event: &v1alpha1::UserMessageRecorded) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.turn_id, "turn_id")?;
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
    require_non_empty(&event.message_id, "message_id")?;
    require_non_empty(&event.model, "model")?;
    require_non_empty(&event.turn_id, "turn_id")?;
    if let Some(settings) = event.settings.as_option() {
        validate_model_settings(settings)?;
    }
    Ok(())
}

fn validate_assistant_message_completed(
    event: &v1alpha1::AssistantMessageCompleted,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.turn_id, "turn_id")?;
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
    require_non_empty(&event.turn_id, "turn_id")?;
    require_known_nonzero(event.reason, "reason")?;
    if let Some(usage) = event.usage.as_option() {
        validate_token_usage(usage, "usage.cost.currency_code")?;
    }
    Ok(())
}

fn validate_tool_call_requested(event: &v1alpha1::ToolCallRequested) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.tool_name, "tool_name")?;
    require_non_empty(&event.turn_id, "turn_id")?;
    require_valid_json(&event.input_json, "input_json")
}

fn validate_tool_call_approved(event: &v1alpha1::ToolCallApproved) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.approved_by, "approved_by")?;
    require_non_empty_when_set(event.turn_id.as_deref(), "turn_id")
}

fn validate_tool_call_denied(event: &v1alpha1::ToolCallDenied) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.denied_by, "denied_by")?;
    require_non_empty_when_set(event.turn_id.as_deref(), "turn_id")
}

fn validate_tool_call_started(event: &v1alpha1::ToolCallStarted) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.turn_id, "turn_id")
}

fn validate_tool_call_completed(event: &v1alpha1::ToolCallCompleted) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.turn_id, "turn_id")?;
    validate_tool_call_result(&event.result)?;
    if let Some(termination) = event.termination.as_option()
        && termination.outcome.is_none()
    {
        return Err(SessionEventValidationError::MissingOneof {
            oneof: "command_termination.outcome",
        });
    }
    if let Some(duration) = event.duration.as_option() {
        require_valid_duration(duration, "duration")?;
    }
    for observation in &event.observed {
        validate_resource_observation(observation)?;
    }
    Ok(())
}

fn validate_tool_call_failed(event: &v1alpha1::ToolCallFailed) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.tool_execution_id, "tool_execution_id")?;
    require_non_empty(&event.error, "error")?;
    require_non_empty(&event.turn_id, "turn_id")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_artifact_recorded(event: &v1alpha1::ArtifactRecorded) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.artifact.artifact_id, "artifact.artifact_id")?;
    require_set_timestamp(&event.artifact.created_at, "artifact.created_at")?;
    let Some(source) = event.artifact.source.as_ref() else {
        return Err(SessionEventValidationError::MissingOneof {
            oneof: "artifact_metadata.source",
        });
    };
    validate_artifact_metadata_source(source)
}

fn validate_file_changed(event: &v1alpha1::FileChanged) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.path, "path")?;
    require_non_empty(&event.tool_call_id, "tool_call_id")?;
    require_non_empty(&event.turn_id, "turn_id")?;
    require_known_nonzero(event.change_kind, "change_kind")?;

    let is_renamed = event.change_kind == v1alpha1::FileChangeKind::Renamed;
    let has_previous_path = event.previous_path.as_deref().is_some_and(|s| !s.is_empty());
    match (is_renamed, has_previous_path) {
        (true, false) => return Err(SessionEventValidationError::RenamedFileChangeMissingPreviousPath),
        (false, true) => return Err(SessionEventValidationError::NonRenamedFileChangeHasPreviousPath),
        _ => {}
    }

    if let Some(before_ref) = event.before_ref.as_option() {
        validate_artifact_ref(before_ref)?;
    }
    if let Some(after_ref) = event.after_ref.as_option() {
        validate_artifact_ref(after_ref)?;
    }
    if let Some(diff) = event.diff.as_option() {
        validate_diff_summary(diff)?;
    }
    Ok(())
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
    let has_previous_attempt_id = event.previous_attempt_id.as_deref().is_some_and(|s| !s.is_empty());
    match (event.attempt_number == 1, has_previous_attempt_id) {
        (true, true) => return Err(SessionEventValidationError::FirstAttemptHasPreviousAttemptId),
        (false, false) => return Err(SessionEventValidationError::RestartAttemptMissingPreviousAttemptId),
        _ => {}
    }
    if let Some(checkpoint) = event.restored_checkpoint.as_option() {
        validate_checkpoint(checkpoint)?;
        if checkpoint.session_execution_plan_digest.as_option() != event.session_execution_plan_digest.as_option() {
            return Err(SessionEventValidationError::RestoredCheckpointPlanDigestMismatch);
        }
    }
    require_set_timestamp(&event.started_at, "started_at")
}

fn validate_execution_attempt_ready(
    event: &v1alpha1::ExecutionAttemptReady,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.execution_attempt_id, "execution_attempt_id")?;
    require_non_empty(&event.ready_attestation_ref, "ready_attestation_ref")?;
    require_digest(&event.ready_attestation_digest, "ready_attestation_digest")?;
    require_set_timestamp(&event.ready_at, "ready_at")
}

fn validate_execution_attempt_ended(
    event: &v1alpha1::ExecutionAttemptEnded,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.execution_attempt_id, "execution_attempt_id")?;
    require_known_nonzero(event.outcome, "outcome")?;
    require_set_timestamp(&event.ended_at, "ended_at")
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
    let Some(outcome) = event.outcome.as_ref() else {
        return Err(SessionEventValidationError::MissingOneof {
            oneof: "operation_outcome_recorded.outcome",
        });
    };
    validate_operation_outcome(outcome)
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
    require_non_empty(&event.text, "text")?;
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
        require_non_empty(&item.content, "items[].content")?;
        require_known_nonzero(item.status, "items[].status")?;
    }
    Ok(())
}

fn validate_provider_tool_intent_rejected(
    event: &v1alpha1::ProviderToolIntentRejected,
) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.rejection_id, "rejection_id")?;
    require_non_empty(&event.message_id, "message_id")?;
    require_non_empty(&event.turn_id, "turn_id")?;
    require_known_nonzero(event.reason, "reason")
}

fn validate_session_renamed(event: &v1alpha1::SessionRenamed) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")?;
    require_non_empty(&event.display_name, "display_name")
}

fn validate_session_archived(event: &v1alpha1::SessionArchived) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")
}

fn validate_session_unarchived(event: &v1alpha1::SessionUnarchived) -> Result<(), SessionEventValidationError> {
    require_non_empty(&event.session_id, "session_id")
}

#[cfg(test)]
mod tests;
