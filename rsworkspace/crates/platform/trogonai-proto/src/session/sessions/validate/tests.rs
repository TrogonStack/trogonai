use buffa::MessageField;

use super::*;

fn digest() -> v1alpha1::Digest {
    v1alpha1::Digest {
        algorithm: "sha256".to_string(),
        value: vec![0u8; 32],
    }
}

fn session_ordinal(value: u64) -> v1alpha1::SessionOrdinal {
    v1alpha1::SessionOrdinal { value }
}

fn assistant_message() -> v1alpha1::CanonicalMessage {
    v1alpha1::CanonicalMessage {
        message_id: "message-1".to_string(),
        role: buffa::EnumValue::from(v1alpha1::MessageRole::Assistant),
        content: vec![v1alpha1::ContentBlock {
            kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
        }],
        model: None,
        usage: MessageField::none(),
        created_at: MessageField::some(valid_timestamp()),
    }
}

fn artifact_ref() -> v1alpha1::ArtifactRef {
    v1alpha1::ArtifactRef {
        artifact_id: "artifact-1".to_string(),
        digest: MessageField::some(digest()),
        size_bytes: 128,
        mime: "text/plain".to_string(),
        preview: None,
        truncated: None,
    }
}

fn checkpoint() -> v1alpha1::Checkpoint {
    v1alpha1::Checkpoint {
        reference: "checkpoint-ref".to_string(),
        checkpoint_type: "full".to_string(),
        digest: MessageField::some(digest()),
        implementation_version: "v1".to_string(),
        checkpoint_id: "checkpoint-1".to_string(),
        producing_execution_attempt_id: "attempt-1".to_string(),
        covers_through: MessageField::some(session_ordinal(1)),
        session_execution_plan_digest: MessageField::some(digest()),
    }
}

fn valid_timestamp() -> buffa_types::google::protobuf::Timestamp {
    buffa_types::google::protobuf::Timestamp::from_unix(1_700_000_000, 0)
}

fn invalid_timestamp() -> buffa_types::google::protobuf::Timestamp {
    let mut timestamp = buffa_types::google::protobuf::Timestamp::from_unix(0, 0);
    timestamp.nanos = -1;
    timestamp
}

fn token_usage_with_currency(currency_code: &str) -> v1alpha1::TokenUsage {
    v1alpha1::TokenUsage {
        input_tokens: None,
        output_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
        cost: MessageField::some(v1alpha1::Cost {
            amount_micros: 1_000_000,
            currency_code: currency_code.to_string(),
            rate_ref: None,
        }),
    }
}

fn user_message_event(content: Vec<v1alpha1::ContentBlock>) -> v1alpha1::SessionEvent {
    v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content,
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    }
}

#[test]
fn validate_session_event_rejects_missing_event_case() {
    let event = v1alpha1::SessionEvent { event: None };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingOneof {
            oneof: "session_event.event"
        })
    );
}

#[test]
fn validate_session_event_accepts_valid_session_started() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionStarted {
                session_id: "session-1".to_string(),
                execution_plan: MessageField::some(v1alpha1::StoredSessionExecutionPlan {
                    plan_bytes: b"plan".to_vec(),
                    plan_digest: MessageField::some(digest()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_closed_rejects_empty_session_id() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionClosed {
                session_id: String::new(),
                result_ref: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier { field: "session_id" })
    );
}

#[test]
fn validate_session_cancelled_rejects_unspecified_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionCancelled {
                session_id: "session-1".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::SessionCancellationReason::Unspecified),
                detail: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "reason" })
    );
}

#[test]
fn validate_session_cancelled_accepts_known_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionCancelled {
                session_id: "session-1".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::SessionCancellationReason::UserRequested),
                detail: None,
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_rejects_missing_content_block_kind() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock { kind: None }],
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingOneof {
            oneof: "content_block.kind"
        })
    );
}

#[test]
fn validate_tool_call_completed_rejects_missing_result_kind() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallCompleted {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: None,
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingOneof {
            oneof: "tool_call_result.kind"
        })
    );
}

#[test]
fn validate_artifact_recorded_rejects_missing_artifact_source() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: None,
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingOneof {
            oneof: "artifact_metadata.source"
        })
    );
}

#[test]
fn validate_operation_outcome_recorded_rejects_missing_outcome() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingOneof {
            oneof: "operation_outcome_recorded.outcome"
        })
    );
}

#[test]
fn validate_user_message_recorded_rejects_assistant_role() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(assistant_message()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnexpectedMessageRole {
            field: "message.role",
            expected: "MESSAGE_ROLE_USER",
        })
    );
}

#[test]
fn validate_assistant_message_completed_rejects_user_role() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageCompleted {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
                    }],
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::some(valid_timestamp()),
                }),
                finish_reason: buffa::EnumValue::from(v1alpha1::FinishReason::EndTurn),
                matched_stop_sequence: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnexpectedMessageRole {
            field: "message.role",
            expected: "MESSAGE_ROLE_ASSISTANT",
        })
    );
}

#[test]
fn validate_file_changed_rejects_renamed_without_previous_path() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::FileChanged {
                session_id: "session-1".to_string(),
                path: "src/new.rs".to_string(),
                change_kind: buffa::EnumValue::from(v1alpha1::FileChangeKind::Renamed),
                previous_path: None,
                before_ref: MessageField::none(),
                after_ref: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::RenamedFileChangeMissingPreviousPath)
    );
}

#[test]
fn validate_file_changed_rejects_non_renamed_with_previous_path() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::FileChanged {
                session_id: "session-1".to_string(),
                path: "src/new.rs".to_string(),
                change_kind: buffa::EnumValue::from(v1alpha1::FileChangeKind::Modified),
                previous_path: Some("src/old.rs".to_string()),
                before_ref: MessageField::none(),
                after_ref: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::NonRenamedFileChangeHasPreviousPath)
    );
}

#[test]
fn validate_file_changed_accepts_renamed_with_previous_path() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::FileChanged {
                session_id: "session-1".to_string(),
                path: "src/new.rs".to_string(),
                change_kind: buffa::EnumValue::from(v1alpha1::FileChangeKind::Renamed),
                previous_path: Some("src/old.rs".to_string()),
                before_ref: MessageField::none(),
                after_ref: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_compacted_rejects_empty_summary_content() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::Compacted {
                session_id: "session-1".to_string(),
                summary_id: "summary-1".to_string(),
                summary_content: String::new(),
                covers_from: MessageField::some(session_ordinal(1)),
                covers_through: MessageField::some(session_ordinal(5)),
                trigger: buffa::EnumValue::from(v1alpha1::CompactionTrigger::Manual),
                guidance: None,
                tokens_before: None,
                tokens_after: None,
                model: None,
                usage: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "summary_content"
        })
    );
}

#[test]
fn validate_compacted_rejects_range_out_of_order() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::Compacted {
                session_id: "session-1".to_string(),
                summary_id: "summary-1".to_string(),
                summary_content: "summary".to_string(),
                covers_from: MessageField::some(session_ordinal(5)),
                covers_through: MessageField::some(session_ordinal(1)),
                trigger: buffa::EnumValue::from(v1alpha1::CompactionTrigger::Manual),
                guidance: None,
                tokens_before: None,
                tokens_after: None,
                model: None,
                usage: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::CompactionRangeOutOfOrder {
            covers_from: 5,
            covers_through: 1
        })
    );
}

#[test]
fn validate_compacted_accepts_in_order_range() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::Compacted {
                session_id: "session-1".to_string(),
                summary_id: "summary-1".to_string(),
                summary_content: "summary".to_string(),
                covers_from: MessageField::some(session_ordinal(1)),
                covers_through: MessageField::some(session_ordinal(5)),
                trigger: buffa::EnumValue::from(v1alpha1::CompactionTrigger::Manual),
                guidance: None,
                tokens_before: None,
                tokens_after: None,
                model: None,
                usage: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_forked_rejects_zero_context_prefix_boundary() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionForked {
                session_id: "session-1".to_string(),
                source_session_id: "session-0".to_string(),
                context_prefix_boundary: MessageField::some(session_ordinal(0)),
                reason: buffa::EnumValue::from(v1alpha1::ForkReason::ManualBranch),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::OrdinalNotPositive {
            field: "context_prefix_boundary"
        })
    );
}

#[test]
fn validate_assistant_message_completed_rejects_missing_matched_stop_sequence() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageCompleted {
                session_id: "session-1".to_string(),
                message: MessageField::some(assistant_message()),
                finish_reason: buffa::EnumValue::from(v1alpha1::FinishReason::StopSequence),
                matched_stop_sequence: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingMatchedStopSequence)
    );
}

#[test]
fn validate_assistant_message_completed_rejects_unexpected_matched_stop_sequence() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageCompleted {
                session_id: "session-1".to_string(),
                message: MessageField::some(assistant_message()),
                finish_reason: buffa::EnumValue::from(v1alpha1::FinishReason::EndTurn),
                matched_stop_sequence: Some("STOP".to_string()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnexpectedMatchedStopSequence)
    );
}

#[test]
fn validate_assistant_message_completed_accepts_stop_sequence_with_match() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageCompleted {
                session_id: "session-1".to_string(),
                message: MessageField::some(assistant_message()),
                finish_reason: buffa::EnumValue::from(v1alpha1::FinishReason::StopSequence),
                matched_stop_sequence: Some("STOP".to_string()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_started_rejects_zero_attempt_number() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 0,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::none(),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::AttemptNumberNotPositive)
    );
}

#[test]
fn validate_execution_attempt_started_accepts_positive_attempt_number() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::none(),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_todo_updated_rejects_empty_item_id() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::TodoUpdated {
                session_id: "session-1".to_string(),
                items: vec![v1alpha1::TodoItem {
                    id: String::new(),
                    content: "write tests".to_string(),
                    status: buffa::EnumValue::from(v1alpha1::TodoStatus::Pending),
                }],
                revision: 1,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyTodoItemId)
    );
}

#[test]
fn validate_todo_updated_rejects_empty_item_content() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::TodoUpdated {
                session_id: "session-1".to_string(),
                items: vec![v1alpha1::TodoItem {
                    id: "todo-1".to_string(),
                    content: String::new(),
                    status: buffa::EnumValue::from(v1alpha1::TodoStatus::Pending),
                }],
                revision: 1,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "items[].content"
        })
    );
}

#[test]
fn validate_todo_updated_rejects_duplicate_item_id() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::TodoUpdated {
                session_id: "session-1".to_string(),
                items: vec![
                    v1alpha1::TodoItem {
                        id: "todo-1".to_string(),
                        content: "first".to_string(),
                        status: buffa::EnumValue::from(v1alpha1::TodoStatus::Pending),
                    },
                    v1alpha1::TodoItem {
                        id: "todo-1".to_string(),
                        content: "second".to_string(),
                        status: buffa::EnumValue::from(v1alpha1::TodoStatus::Completed),
                    },
                ],
                revision: 1,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::DuplicateTodoItemId {
            id: "todo-1".to_string()
        })
    );
}

#[test]
fn validate_todo_updated_rejects_zero_revision() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::TodoUpdated {
                session_id: "session-1".to_string(),
                items: Vec::new(),
                revision: 0,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::TodoRevisionNotPositive)
    );
}

#[test]
fn validate_todo_updated_accepts_unique_ids_and_positive_revision() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::TodoUpdated {
                session_id: "session-1".to_string(),
                items: vec![
                    v1alpha1::TodoItem {
                        id: "todo-1".to_string(),
                        content: "first".to_string(),
                        status: buffa::EnumValue::from(v1alpha1::TodoStatus::Pending),
                    },
                    v1alpha1::TodoItem {
                        id: "todo-2".to_string(),
                        content: "second".to_string(),
                        status: buffa::EnumValue::from(v1alpha1::TodoStatus::InProgress),
                    },
                ],
                revision: 1,
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_redaction_applied_rejects_empty_redacted_event_ids() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::RedactionApplied {
                session_id: "session-1".to_string(),
                redacted_event_ids: Vec::new(),
                reason: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyRedactedEventIds)
    );
}

#[test]
fn validate_redaction_applied_accepts_non_empty_redacted_event_ids() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::RedactionApplied {
                session_id: "session-1".to_string(),
                redacted_event_ids: vec!["event-1".to_string()],
                reason: None,
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_started_rejects_empty_plan_bytes() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionStarted {
                session_id: "session-1".to_string(),
                execution_plan: MessageField::some(v1alpha1::StoredSessionExecutionPlan {
                    plan_bytes: Vec::new(),
                    plan_digest: MessageField::some(v1alpha1::Digest {
                        algorithm: "sha256".to_string(),
                        value: vec![0u8; 32],
                    }),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "execution_plan.plan_bytes"
        })
    );
}

#[test]
fn validate_session_started_rejects_unsupported_digest_algorithm() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionStarted {
                session_id: "session-1".to_string(),
                execution_plan: MessageField::some(v1alpha1::StoredSessionExecutionPlan {
                    plan_bytes: b"plan".to_vec(),
                    plan_digest: MessageField::some(v1alpha1::Digest {
                        algorithm: "sha512".to_string(),
                        value: Vec::new(),
                    }),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnsupportedDigestAlgorithm {
            field: "execution_plan.plan_digest"
        })
    );
}

#[test]
fn validate_session_started_rejects_empty_digest_algorithm() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionStarted {
                session_id: "session-1".to_string(),
                execution_plan: MessageField::some(v1alpha1::StoredSessionExecutionPlan {
                    plan_bytes: b"plan".to_vec(),
                    plan_digest: MessageField::some(v1alpha1::Digest {
                        algorithm: String::new(),
                        value: vec![0u8; 32],
                    }),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyDigestAlgorithm {
            field: "execution_plan.plan_digest"
        })
    );
}

#[test]
fn validate_session_started_rejects_wrong_length_sha256_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionStarted {
                session_id: "session-1".to_string(),
                execution_plan: MessageField::some(v1alpha1::StoredSessionExecutionPlan {
                    plan_bytes: b"plan".to_vec(),
                    plan_digest: MessageField::some(v1alpha1::Digest {
                        algorithm: "sha256".to_string(),
                        value: vec![0u8; 4],
                    }),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::Sha256DigestWrongLength {
            field: "execution_plan.plan_digest",
            actual: 4
        })
    );
}

#[test]
fn validate_session_started_accepts_valid_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionStarted {
                session_id: "session-1".to_string(),
                execution_plan: MessageField::some(v1alpha1::StoredSessionExecutionPlan {
                    plan_bytes: b"plan".to_vec(),
                    plan_digest: MessageField::some(digest()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_failed_rejects_unspecified_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionFailed {
                session_id: "session-1".to_string(),
                detail: Some("boom".to_string()),
                reason: buffa::EnumValue::from(v1alpha1::SessionFailureReason::Unspecified),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "reason" })
    );
}

#[test]
fn validate_session_failed_accepts_known_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionFailed {
                session_id: "session-1".to_string(),
                detail: Some("boom".to_string()),
                reason: buffa::EnumValue::from(v1alpha1::SessionFailureReason::ExecutionError),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_failed_accepts_empty_detail() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionFailed {
                session_id: "session-1".to_string(),
                detail: None,
                reason: buffa::EnumValue::from(v1alpha1::SessionFailureReason::Timeout),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_hidden_rejects_unspecified_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionHidden {
                session_id: "session-1".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::SessionHiddenReason::Unspecified),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "reason" })
    );
}

#[test]
fn validate_session_hidden_accepts_known_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionHidden {
                session_id: "session-1".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::SessionHiddenReason::UserRequested),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_forked_rejects_unspecified_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionForked {
                session_id: "session-1".to_string(),
                source_session_id: "session-0".to_string(),
                context_prefix_boundary: MessageField::some(session_ordinal(3)),
                reason: buffa::EnumValue::from(v1alpha1::ForkReason::Unspecified),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "reason" })
    );
}

#[test]
fn validate_session_forked_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionForked {
                session_id: "session-1".to_string(),
                source_session_id: "session-0".to_string(),
                context_prefix_boundary: MessageField::some(session_ordinal(3)),
                reason: buffa::EnumValue::from(v1alpha1::ForkReason::ManualBranch),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_rewound_rejects_zero_keep_through() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionRewound {
                session_id: "session-1".to_string(),
                keep_through: MessageField::some(session_ordinal(0)),
                reason: buffa::EnumValue::from(v1alpha1::RewindReason::Manual),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::OrdinalNotPositive { field: "keep_through" })
    );
}

#[test]
fn validate_session_rewound_rejects_unspecified_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionRewound {
                session_id: "session-1".to_string(),
                keep_through: MessageField::some(session_ordinal(2)),
                reason: buffa::EnumValue::from(v1alpha1::RewindReason::Unspecified),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "reason" })
    );
}

#[test]
fn validate_session_rewound_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionRewound {
                session_id: "session-1".to_string(),
                keep_through: MessageField::some(session_ordinal(2)),
                reason: buffa::EnumValue::from(v1alpha1::RewindReason::Manual),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_assistant_message_failed_rejects_unspecified_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageFailed {
                session_id: "session-1".to_string(),
                message_id: "message-1".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::AssistantMessageFailureReason::Unspecified),
                detail: None,
                usage: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "reason" })
    );
}

#[test]
fn validate_assistant_message_failed_accepts_known_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageFailed {
                session_id: "session-1".to_string(),
                message_id: "message-1".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::AssistantMessageFailureReason::Error),
                detail: None,
                usage: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_tool_call_failed_rejects_unspecified_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallFailed {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                error: "boom".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::ToolCallFailureReason::Unspecified),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "reason" })
    );
}

#[test]
fn validate_tool_call_failed_rejects_empty_error() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallFailed {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                error: String::new(),
                reason: buffa::EnumValue::from(v1alpha1::ToolCallFailureReason::Error),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier { field: "error" })
    );
}

#[test]
fn validate_tool_call_failed_accepts_known_reason() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallFailed {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                error: "boom".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::ToolCallFailureReason::Error),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_ready_rejects_wrong_length_sha256_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptReady {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                ready_attestation_ref: "ready-ref".to_string(),
                ready_attestation_digest: MessageField::some(v1alpha1::Digest {
                    algorithm: "sha256".to_string(),
                    value: vec![0u8; 4],
                }),
                ready_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::Sha256DigestWrongLength {
            field: "ready_attestation_digest",
            actual: 4
        })
    );
}

#[test]
fn validate_execution_attempt_ready_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptReady {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                ready_attestation_ref: "ready-ref".to_string(),
                ready_attestation_digest: MessageField::some(digest()),
                ready_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_ended_rejects_unspecified_outcome() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptEnded {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                outcome: buffa::EnumValue::from(v1alpha1::AttemptOutcome::Unspecified),
                detail: None,
                ended_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "outcome" })
    );
}

#[test]
fn validate_execution_attempt_ended_accepts_known_outcome() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptEnded {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                outcome: buffa::EnumValue::from(v1alpha1::AttemptOutcome::Failed),
                detail: None,
                ended_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_delegation_dispatched_rejects_unspecified_cascade_policy() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::DelegationDispatched {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                child_session_id: "session-2".to_string(),
                cascade_policy: buffa::EnumValue::from(v1alpha1::CascadePolicy::Unspecified),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum {
            field: "cascade_policy"
        })
    );
}

#[test]
fn validate_delegation_dispatched_accepts_known_cascade_policy() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::DelegationDispatched {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                child_session_id: "session-2".to_string(),
                cascade_policy: buffa::EnumValue::from(v1alpha1::CascadePolicy::CascadeOnParentTerminal),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_parent_linked_rejects_zero_parent_dispatched_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ParentLinked {
                session_id: "session-1".to_string(),
                parent_session_id: "session-0".to_string(),
                parent_dispatched_at: MessageField::some(session_ordinal(0)),
                cascade_policy: buffa::EnumValue::from(v1alpha1::CascadePolicy::CascadeOnParentTerminal),
                operation_id: "operation-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::OrdinalNotPositive {
            field: "parent_dispatched_at"
        })
    );
}

#[test]
fn validate_parent_linked_rejects_unspecified_cascade_policy() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ParentLinked {
                session_id: "session-1".to_string(),
                parent_session_id: "session-0".to_string(),
                parent_dispatched_at: MessageField::some(session_ordinal(1)),
                cascade_policy: buffa::EnumValue::from(v1alpha1::CascadePolicy::Unspecified),
                operation_id: "operation-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum {
            field: "cascade_policy"
        })
    );
}

#[test]
fn validate_parent_linked_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ParentLinked {
                session_id: "session-1".to_string(),
                parent_session_id: "session-0".to_string(),
                parent_dispatched_at: MessageField::some(session_ordinal(1)),
                cascade_policy: buffa::EnumValue::from(v1alpha1::CascadePolicy::CascadeOnParentTerminal),
                operation_id: "operation-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_parent_terminated_rejects_unspecified_cause() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ParentTerminated {
                session_id: "session-1".to_string(),
                parent_session_id: "session-0".to_string(),
                cause: buffa::EnumValue::from(v1alpha1::ParentTerminalCause::Unspecified),
                triggering_event_id: "event-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "cause" })
    );
}

#[test]
fn validate_parent_terminated_accepts_known_cause() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ParentTerminated {
                session_id: "session-1".to_string(),
                parent_session_id: "session-0".to_string(),
                cause: buffa::EnumValue::from(v1alpha1::ParentTerminalCause::Closed),
                triggering_event_id: "event-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_parent_history_invalidated_rejects_zero_parent_keep_through() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ParentHistoryInvalidated {
                session_id: "session-1".to_string(),
                parent_session_id: "session-0".to_string(),
                parent_keep_through: MessageField::some(session_ordinal(0)),
                triggering_event_id: "event-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::OrdinalNotPositive {
            field: "parent_keep_through"
        })
    );
}

#[test]
fn validate_parent_history_invalidated_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ParentHistoryInvalidated {
                session_id: "session-1".to_string(),
                parent_session_id: "session-0".to_string(),
                parent_keep_through: MessageField::some(session_ordinal(4)),
                triggering_event_id: "event-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_external_delegation_dispatched_rejects_wrong_length_sha256_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExternalDelegationDispatched {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                delegate_reference: "delegate-ref".to_string(),
                authenticated_remote_subject: "subject-1".to_string(),
                authorization_reference: "authz-ref".to_string(),
                request_digest: MessageField::some(v1alpha1::Digest {
                    algorithm: "sha256".to_string(),
                    value: vec![0u8; 4],
                }),
                correlation_id: "correlation-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::Sha256DigestWrongLength {
            field: "request_digest",
            actual: 4
        })
    );
}

#[test]
fn validate_external_delegation_dispatched_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExternalDelegationDispatched {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                delegate_reference: "delegate-ref".to_string(),
                authenticated_remote_subject: "subject-1".to_string(),
                authorization_reference: "authz-ref".to_string(),
                request_digest: MessageField::some(digest()),
                correlation_id: "correlation-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_operation_reserved_rejects_unspecified_operation_kind() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationReserved {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                request_digest: MessageField::some(digest()),
                operation_kind: buffa::EnumValue::from(v1alpha1::OperationKind::Unspecified),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum {
            field: "operation_kind"
        })
    );
}

#[test]
fn validate_operation_reserved_rejects_wrong_length_sha256_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationReserved {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                request_digest: MessageField::some(v1alpha1::Digest {
                    algorithm: "sha256".to_string(),
                    value: vec![0u8; 4],
                }),
                operation_kind: buffa::EnumValue::from(v1alpha1::OperationKind::Tool),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::Sha256DigestWrongLength {
            field: "request_digest",
            actual: 4
        })
    );
}

#[test]
fn validate_operation_reserved_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationReserved {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                request_digest: MessageField::some(digest()),
                operation_kind: buffa::EnumValue::from(v1alpha1::OperationKind::Tool),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_system_notice_recorded_rejects_unspecified_level() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SystemNoticeRecorded {
                session_id: "session-1".to_string(),
                level: buffa::EnumValue::from(v1alpha1::NoticeLevel::Unspecified),
                text: "notice".to_string(),
                tool_call_id: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::UnspecifiedEnum { field: "level" })
    );
}

#[test]
fn validate_system_notice_recorded_accepts_known_level() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SystemNoticeRecorded {
                session_id: "session-1".to_string(),
                level: buffa::EnumValue::from(v1alpha1::NoticeLevel::Info),
                text: "notice".to_string(),
                tool_call_id: None,
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_system_notice_recorded_rejects_empty_text() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SystemNoticeRecorded {
                session_id: "session-1".to_string(),
                level: buffa::EnumValue::from(v1alpha1::NoticeLevel::Info),
                text: String::new(),
                tool_call_id: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier { field: "text" })
    );
}

#[test]
fn validate_assistant_message_started_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageStarted {
                session_id: "session-1".to_string(),
                message_id: "message-1".to_string(),
                model: "model".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_assistant_message_started_rejects_empty_model() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageStarted {
                session_id: "session-1".to_string(),
                message_id: "message-1".to_string(),
                model: String::new(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier { field: "model" })
    );
}

#[test]
fn validate_tool_call_requested_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallRequested {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                tool_name: "search".to_string(),
                input_json: "{}".to_string(),
                parent_tool_use_id: None,
                operation_id: None,
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_tool_call_approved_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallApproved {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                approved_by: "user-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_tool_call_denied_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallDenied {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                denied_by: "user-1".to_string(),
                reason: None,
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_tool_call_started_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallStarted {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_delegation_detached_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::DelegationDetached {
                session_id: "session-1".to_string(),
                child_session_id: "session-2".to_string(),
                reason: None,
                detach_operation_id: "operation-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_parent_detached_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ParentDetached {
                session_id: "session-1".to_string(),
                parent_session_id: "session-0".to_string(),
                detach_operation_id: "operation-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_operation_cancellation_requested_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationCancellationRequested {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                reason: None,
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_artifact_erased_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactErased {
                session_id: "session-1".to_string(),
                artifact_id: "artifact-1".to_string(),
                reason: None,
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_renamed_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionRenamed {
                session_id: "session-1".to_string(),
                display_name: "New title".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_renamed_rejects_empty_title() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionRenamed {
                session_id: "session-1".to_string(),
                display_name: String::new(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier { field: "display_name" })
    );
}

#[test]
fn validate_session_archived_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionArchived {
                session_id: "session-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_unarchived_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionUnarchived {
                session_id: "session-1".to_string(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
                    }],
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_tool_call_completed_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallCompleted {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: Some(v1alpha1::tool_call_result::Kind::Text(Box::new(
                        v1alpha1::TextToolResult {
                            content: "done".to_string(),
                            truncated: None,
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_artifact_recorded_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::Stored(Box::new(
                        v1alpha1::StoredArtifact {
                            digest: MessageField::some(digest()),
                            size_bytes: 128,
                            storage_ref: "blob://artifact-1".to_string(),
                            mime: "text/plain".to_string(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_operation_outcome_recorded_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Succeeded(Box::new(
                    v1alpha1::OperationSucceeded {
                        response_digest: MessageField::some(digest()),
                        response_ref: MessageField::some(artifact_ref()),
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_operation_outcome_recorded_accepts_succeeded_without_response_ref() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Succeeded(Box::new(
                    v1alpha1::OperationSucceeded {
                        response_digest: MessageField::some(digest()),
                        response_ref: MessageField::none(),
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_checkpoint_produced_accepts_valid_event() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::CheckpointProduced {
                session_id: "session-1".to_string(),
                checkpoint: MessageField::some(checkpoint()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_started_accepts_valid_restored_checkpoint() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::some(checkpoint()),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_started_rejects_first_attempt_with_previous_attempt_id() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: Some("attempt-0".to_string()),
                restored_checkpoint: MessageField::none(),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::FirstAttemptHasPreviousAttemptId)
    );
}

#[test]
fn validate_execution_attempt_started_rejects_restart_without_previous_attempt_id() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-2".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 2,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::none(),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::RestartAttemptMissingPreviousAttemptId)
    );
}

#[test]
fn validate_execution_attempt_started_accepts_restart_with_previous_attempt_id() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-2".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 2,
                previous_attempt_id: Some("attempt-1".to_string()),
                restored_checkpoint: MessageField::none(),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_started_rejects_invalid_restored_checkpoint() {
    let mut broken_checkpoint = checkpoint();
    broken_checkpoint.reference = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::some(broken_checkpoint),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "checkpoint.reference"
        })
    );
}

#[test]
fn validate_redaction_applied_rejects_empty_first_event_id() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::RedactionApplied {
                session_id: "session-1".to_string(),
                redacted_event_ids: vec![String::new(), "event-2".to_string()],
                reason: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "redacted_event_ids[0]"
        })
    );
}

#[test]
fn validate_redaction_applied_rejects_empty_non_first_event_id() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::RedactionApplied {
                session_id: "session-1".to_string(),
                redacted_event_ids: vec!["event-1".to_string(), String::new()],
                reason: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "redacted_event_ids[n]"
        })
    );
}

#[test]
fn validate_execution_attempt_started_rejects_checkpoint_with_empty_producing_execution_attempt_id() {
    let mut broken_checkpoint = checkpoint();
    broken_checkpoint.producing_execution_attempt_id = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::some(broken_checkpoint),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "checkpoint.producing_execution_attempt_id"
        })
    );
}

#[test]
fn validate_execution_attempt_started_rejects_checkpoint_with_invalid_session_execution_plan_digest() {
    let mut broken_checkpoint = checkpoint();
    broken_checkpoint.session_execution_plan_digest = MessageField::some(v1alpha1::Digest {
        algorithm: String::new(),
        value: vec![0u8; 32],
    });

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::some(broken_checkpoint),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyDigestAlgorithm {
            field: "checkpoint.session_execution_plan_digest"
        })
    );
}

#[test]
fn validate_user_message_recorded_accepts_artifact_ref_content_block() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::ArtifactRef(Box::new(artifact_ref()))),
                    }],
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_rejects_invalid_artifact_ref_content_block() {
    let mut broken_artifact_ref = artifact_ref();
    broken_artifact_ref.mime = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::ArtifactRef(Box::new(
                            broken_artifact_ref,
                        ))),
                    }],
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_ref.mime"
        })
    );
}

#[test]
fn validate_tool_call_completed_accepts_artifact_ref_result() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallCompleted {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: Some(v1alpha1::tool_call_result::Kind::ArtifactRef(Box::new(artifact_ref()))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_tool_call_completed_rejects_invalid_artifact_ref_result() {
    let mut broken_artifact_ref = artifact_ref();
    broken_artifact_ref.artifact_id = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallCompleted {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: Some(v1alpha1::tool_call_result::Kind::ArtifactRef(Box::new(
                        broken_artifact_ref,
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_ref.artifact_id"
        })
    );
}

#[test]
fn validate_file_changed_accepts_valid_before_and_after_ref() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::FileChanged {
                session_id: "session-1".to_string(),
                path: "src/new.rs".to_string(),
                change_kind: buffa::EnumValue::from(v1alpha1::FileChangeKind::Modified),
                previous_path: None,
                before_ref: MessageField::some(artifact_ref()),
                after_ref: MessageField::some(artifact_ref()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_file_changed_rejects_invalid_before_ref() {
    let mut broken_artifact_ref = artifact_ref();
    broken_artifact_ref.mime = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::FileChanged {
                session_id: "session-1".to_string(),
                path: "src/new.rs".to_string(),
                change_kind: buffa::EnumValue::from(v1alpha1::FileChangeKind::Modified),
                previous_path: None,
                before_ref: MessageField::some(broken_artifact_ref),
                after_ref: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_ref.mime"
        })
    );
}

#[test]
fn validate_file_changed_rejects_invalid_after_ref() {
    let mut broken_artifact_ref = artifact_ref();
    broken_artifact_ref.mime = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::FileChanged {
                session_id: "session-1".to_string(),
                path: "src/new.rs".to_string(),
                change_kind: buffa::EnumValue::from(v1alpha1::FileChangeKind::Modified),
                previous_path: None,
                before_ref: MessageField::none(),
                after_ref: MessageField::some(broken_artifact_ref),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_ref.mime"
        })
    );
}

#[test]
fn validate_session_closed_accepts_valid_result_ref() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionClosed {
                session_id: "session-1".to_string(),
                result_ref: MessageField::some(artifact_ref()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_closed_accepts_valid_event_without_result_ref() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionClosed {
                session_id: "session-1".to_string(),
                result_ref: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_session_closed_rejects_invalid_result_ref() {
    let mut broken_artifact_ref = artifact_ref();
    broken_artifact_ref.artifact_id = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::SessionClosed {
                session_id: "session-1".to_string(),
                result_ref: MessageField::some(broken_artifact_ref),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_ref.artifact_id"
        })
    );
}

#[test]
fn validate_artifact_recorded_accepts_external_source() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::External(Box::new(
                        v1alpha1::ExternalArtifact {
                            source_url: "https://example.com/artifact-1".to_string(),
                            source_encoding: None,
                            declared_mime: None,
                            fetched_at: MessageField::none(),
                            content_digest: MessageField::some(digest()),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_artifact_recorded_accepts_external_source_without_content_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::External(Box::new(
                        v1alpha1::ExternalArtifact {
                            source_url: "https://example.com/artifact-1".to_string(),
                            source_encoding: None,
                            declared_mime: None,
                            fetched_at: MessageField::none(),
                            content_digest: MessageField::none(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_artifact_recorded_rejects_external_source_empty_source_url() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::External(Box::new(
                        v1alpha1::ExternalArtifact {
                            source_url: String::new(),
                            source_encoding: None,
                            declared_mime: None,
                            fetched_at: MessageField::none(),
                            content_digest: MessageField::none(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_metadata.external.source_url"
        })
    );
}

#[test]
fn validate_artifact_recorded_rejects_external_source_invalid_content_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::External(Box::new(
                        v1alpha1::ExternalArtifact {
                            source_url: "https://example.com/artifact-1".to_string(),
                            source_encoding: None,
                            declared_mime: None,
                            fetched_at: MessageField::none(),
                            content_digest: MessageField::some(v1alpha1::Digest {
                                algorithm: String::new(),
                                value: Vec::new(),
                            }),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyDigestAlgorithm {
            field: "artifact_metadata.external.content_digest"
        })
    );
}

#[test]
fn validate_artifact_recorded_rejects_stored_source_invalid_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::Stored(Box::new(
                        v1alpha1::StoredArtifact {
                            digest: MessageField::none(),
                            size_bytes: 128,
                            storage_ref: "blob://artifact-1".to_string(),
                            mime: "text/plain".to_string(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyDigestAlgorithm {
            field: "artifact_metadata.stored.digest"
        })
    );
}

#[test]
fn validate_artifact_recorded_rejects_stored_source_empty_storage_ref() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::Stored(Box::new(
                        v1alpha1::StoredArtifact {
                            digest: MessageField::some(digest()),
                            size_bytes: 128,
                            storage_ref: String::new(),
                            mime: "text/plain".to_string(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_metadata.stored.storage_ref"
        })
    );
}

#[test]
fn validate_artifact_recorded_rejects_stored_source_empty_mime() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::Stored(Box::new(
                        v1alpha1::StoredArtifact {
                            digest: MessageField::some(digest()),
                            size_bytes: 128,
                            storage_ref: "blob://artifact-1".to_string(),
                            mime: String::new(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_metadata.stored.mime"
        })
    );
}

#[test]
fn validate_operation_outcome_recorded_rejects_invalid_succeeded_response_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Succeeded(Box::new(
                    v1alpha1::OperationSucceeded {
                        response_digest: MessageField::some(v1alpha1::Digest {
                            algorithm: String::new(),
                            value: Vec::new(),
                        }),
                        response_ref: MessageField::none(),
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyDigestAlgorithm {
            field: "operation_outcome_recorded.succeeded.response_digest"
        })
    );
}

#[test]
fn validate_operation_outcome_recorded_rejects_invalid_succeeded_response_ref() {
    let mut broken_artifact_ref = artifact_ref();
    broken_artifact_ref.mime = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Succeeded(Box::new(
                    v1alpha1::OperationSucceeded {
                        response_digest: MessageField::some(digest()),
                        response_ref: MessageField::some(broken_artifact_ref),
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "artifact_ref.mime"
        })
    );
}

#[test]
fn validate_operation_outcome_recorded_accepts_failed_outcome() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Failed(Box::new(
                    v1alpha1::OperationFailed {
                        detail: "operation failed".to_string(),
                        failure_digest: MessageField::some(digest()),
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_operation_outcome_recorded_accepts_failed_without_failure_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Failed(Box::new(
                    v1alpha1::OperationFailed {
                        detail: "operation failed".to_string(),
                        failure_digest: MessageField::none(),
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_operation_outcome_recorded_rejects_failed_empty_detail() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Failed(Box::new(
                    v1alpha1::OperationFailed {
                        detail: String::new(),
                        failure_digest: MessageField::none(),
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "operation_outcome_recorded.failed.detail"
        })
    );
}

#[test]
fn validate_operation_outcome_recorded_rejects_failed_invalid_failure_digest() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Failed(Box::new(
                    v1alpha1::OperationFailed {
                        detail: "operation failed".to_string(),
                        failure_digest: MessageField::some(v1alpha1::Digest {
                            algorithm: "sha256".to_string(),
                            value: vec![0u8; 4],
                        }),
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::Sha256DigestWrongLength {
            field: "operation_outcome_recorded.failed.failure_digest",
            actual: 4,
        })
    );
}

#[test]
fn validate_operation_outcome_recorded_accepts_cancelled_outcome() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Cancelled(Box::new(
                    v1alpha1::OperationCancelled {
                        cancelled_by: "user-1".to_string(),
                        reason: None,
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_operation_outcome_recorded_rejects_cancelled_empty_cancelled_by() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Cancelled(Box::new(
                    v1alpha1::OperationCancelled {
                        cancelled_by: String::new(),
                        reason: None,
                    },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "operation_outcome_recorded.cancelled.cancelled_by"
        })
    );
}

#[test]
fn validate_operation_outcome_recorded_accepts_unknown_outcome() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::OperationOutcomeRecorded {
                session_id: "session-1".to_string(),
                operation_id: "operation-1".to_string(),
                outcome: Some(v1alpha1::operation_outcome_recorded::Outcome::Unknown(Box::new(
                    v1alpha1::OperationUnknown { detail: None },
                ))),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_accepts_thinking_content_block() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::Thinking(Box::new(
            v1alpha1::ThinkingBlock {
                text: "reasoning".to_string(),
                signature: None,
            },
        ))),
    }]);

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_rejects_empty_thinking_text() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::Thinking(Box::new(
            v1alpha1::ThinkingBlock {
                text: String::new(),
                signature: None,
            },
        ))),
    }]);

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "content_block.thinking.text"
        })
    );
}

#[test]
fn validate_user_message_recorded_accepts_tool_use_content_block() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::ToolUse(Box::new(
            v1alpha1::ToolUseBlock {
                id: "tool-use-1".to_string(),
                name: "search".to_string(),
                input_json: "{}".to_string(),
                parent_tool_use_id: None,
            },
        ))),
    }]);

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_rejects_empty_tool_use_id() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::ToolUse(Box::new(
            v1alpha1::ToolUseBlock {
                id: String::new(),
                name: "search".to_string(),
                input_json: "{}".to_string(),
                parent_tool_use_id: None,
            },
        ))),
    }]);

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "content_block.tool_use.id"
        })
    );
}

#[test]
fn validate_user_message_recorded_rejects_empty_tool_use_name() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::ToolUse(Box::new(
            v1alpha1::ToolUseBlock {
                id: "tool-use-1".to_string(),
                name: String::new(),
                input_json: "{}".to_string(),
                parent_tool_use_id: None,
            },
        ))),
    }]);

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "content_block.tool_use.name"
        })
    );
}

#[test]
fn validate_user_message_recorded_rejects_invalid_tool_use_input_json() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::ToolUse(Box::new(
            v1alpha1::ToolUseBlock {
                id: "tool-use-1".to_string(),
                name: "search".to_string(),
                input_json: "not json".to_string(),
                parent_tool_use_id: None,
            },
        ))),
    }]);

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidJson {
            field: "content_block.tool_use.input_json"
        })
    );
}

#[test]
fn validate_user_message_recorded_accepts_tool_result_content_block() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::ToolResult(Box::new(
            v1alpha1::ToolResultBlock {
                tool_use_id: "tool-use-1".to_string(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: Some(v1alpha1::tool_call_result::Kind::Text(Box::new(
                        v1alpha1::TextToolResult {
                            content: "done".to_string(),
                            truncated: None,
                        },
                    ))),
                }),
            },
        ))),
    }]);

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_rejects_empty_tool_result_tool_use_id() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::ToolResult(Box::new(
            v1alpha1::ToolResultBlock {
                tool_use_id: String::new(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: Some(v1alpha1::tool_call_result::Kind::Text(Box::new(
                        v1alpha1::TextToolResult {
                            content: "done".to_string(),
                            truncated: None,
                        },
                    ))),
                }),
            },
        ))),
    }]);

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "content_block.tool_result.tool_use_id"
        })
    );
}

#[test]
fn validate_user_message_recorded_rejects_tool_result_missing_kind() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::ToolResult(Box::new(
            v1alpha1::ToolResultBlock {
                tool_use_id: "tool-use-1".to_string(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: None,
                }),
            },
        ))),
    }]);

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingOneof {
            oneof: "tool_call_result.kind"
        })
    );
}

#[test]
fn validate_user_message_recorded_rejects_tool_result_empty_text_content() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::ToolResult(Box::new(
            v1alpha1::ToolResultBlock {
                tool_use_id: "tool-use-1".to_string(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: Some(v1alpha1::tool_call_result::Kind::Text(Box::new(
                        v1alpha1::TextToolResult {
                            content: String::new(),
                            truncated: None,
                        },
                    ))),
                }),
            },
        ))),
    }]);

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "tool_call_result.text.content"
        })
    );
}

#[test]
fn validate_user_message_recorded_accepts_redacted_thinking_content_block() {
    let event = user_message_event(vec![v1alpha1::ContentBlock {
        kind: Some(v1alpha1::content_block::Kind::RedactedThinking(vec![1, 2, 3])),
    }]);

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_tool_call_completed_rejects_empty_text_result_content() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallCompleted {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                result: MessageField::some(v1alpha1::ToolCallResult {
                    status: buffa::EnumValue::from(v1alpha1::ToolCallResultStatus::Success),
                    kind: Some(v1alpha1::tool_call_result::Kind::Text(Box::new(
                        v1alpha1::TextToolResult {
                            content: String::new(),
                            truncated: None,
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "tool_call_result.text.content"
        })
    );
}

#[test]
fn validate_tool_call_requested_rejects_invalid_input_json() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ToolCallRequested {
                session_id: "session-1".to_string(),
                tool_call_id: "tool-call-1".to_string(),
                tool_execution_id: "tool-exec-1".to_string(),
                tool_name: "search".to_string(),
                input_json: "{not json".to_string(),
                parent_tool_use_id: None,
                operation_id: None,
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidJson { field: "input_json" })
    );
}

#[test]
fn validate_execution_attempt_started_accepts_valid_started_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::none(),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_started_rejects_invalid_started_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::none(),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::some(invalid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidTimestamp { field: "started_at" })
    );
}

#[test]
fn validate_execution_attempt_started_rejects_missing_started_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptStarted {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                session_execution_plan_digest: MessageField::some(digest()),
                attempt_number: 1,
                previous_attempt_id: None,
                restored_checkpoint: MessageField::none(),
                resume_cursor: None,
                host_artifact_ref: "host-ref".to_string(),
                host_artifact_digest: MessageField::some(digest()),
                authenticated_remote_subject: None,
                isolation_placement: None,
                started_at: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingTimestamp { field: "started_at" })
    );
}

#[test]
fn validate_execution_attempt_ready_accepts_valid_ready_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptReady {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                ready_attestation_ref: "ready-ref".to_string(),
                ready_attestation_digest: MessageField::some(digest()),
                ready_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_ready_rejects_invalid_ready_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptReady {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                ready_attestation_ref: "ready-ref".to_string(),
                ready_attestation_digest: MessageField::some(digest()),
                ready_at: MessageField::some(invalid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidTimestamp { field: "ready_at" })
    );
}

#[test]
fn validate_execution_attempt_ready_rejects_missing_ready_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptReady {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                ready_attestation_ref: "ready-ref".to_string(),
                ready_attestation_digest: MessageField::some(digest()),
                ready_at: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingTimestamp { field: "ready_at" })
    );
}

#[test]
fn validate_execution_attempt_ended_accepts_valid_ended_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptEnded {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                outcome: buffa::EnumValue::from(v1alpha1::AttemptOutcome::Failed),
                detail: None,
                ended_at: MessageField::some(valid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_execution_attempt_ended_rejects_invalid_ended_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptEnded {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                outcome: buffa::EnumValue::from(v1alpha1::AttemptOutcome::Failed),
                detail: None,
                ended_at: MessageField::some(invalid_timestamp()),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidTimestamp { field: "ended_at" })
    );
}

#[test]
fn validate_execution_attempt_ended_rejects_missing_ended_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ExecutionAttemptEnded {
                session_id: "session-1".to_string(),
                execution_attempt_id: "attempt-1".to_string(),
                outcome: buffa::EnumValue::from(v1alpha1::AttemptOutcome::Failed),
                detail: None,
                ended_at: MessageField::none(),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingTimestamp { field: "ended_at" })
    );
}

#[test]
fn validate_user_message_recorded_accepts_valid_created_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
                    }],
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_rejects_invalid_created_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
                    }],
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::some(invalid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidTimestamp {
            field: "message.created_at"
        })
    );
}

#[test]
fn validate_user_message_recorded_rejects_missing_created_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
                    }],
                    model: None,
                    usage: MessageField::none(),
                    created_at: MessageField::none(),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingTimestamp {
            field: "message.created_at"
        })
    );
}

#[test]
fn validate_artifact_recorded_accepts_valid_created_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::Stored(Box::new(
                        v1alpha1::StoredArtifact {
                            digest: MessageField::some(digest()),
                            size_bytes: 128,
                            storage_ref: "blob://artifact-1".to_string(),
                            mime: "text/plain".to_string(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_artifact_recorded_rejects_invalid_created_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(invalid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::Stored(Box::new(
                        v1alpha1::StoredArtifact {
                            digest: MessageField::some(digest()),
                            size_bytes: 128,
                            storage_ref: "blob://artifact-1".to_string(),
                            mime: "text/plain".to_string(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidTimestamp {
            field: "artifact.created_at"
        })
    );
}

#[test]
fn validate_artifact_recorded_rejects_missing_created_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::none(),
                    source: Some(v1alpha1::artifact_metadata::Source::Stored(Box::new(
                        v1alpha1::StoredArtifact {
                            digest: MessageField::some(digest()),
                            size_bytes: 128,
                            storage_ref: "blob://artifact-1".to_string(),
                            mime: "text/plain".to_string(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::MissingTimestamp {
            field: "artifact.created_at"
        })
    );
}

#[test]
fn validate_artifact_recorded_accepts_external_source_with_valid_fetched_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::External(Box::new(
                        v1alpha1::ExternalArtifact {
                            source_url: "https://example.com/artifact-1".to_string(),
                            source_encoding: None,
                            declared_mime: None,
                            fetched_at: MessageField::some(valid_timestamp()),
                            content_digest: MessageField::none(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_artifact_recorded_rejects_external_source_invalid_fetched_at() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::ArtifactRecorded {
                session_id: "session-1".to_string(),
                artifact: MessageField::some(v1alpha1::ArtifactMetadata {
                    artifact_id: "artifact-1".to_string(),
                    preview: None,
                    truncated: None,
                    created_at: MessageField::some(valid_timestamp()),
                    source: Some(v1alpha1::artifact_metadata::Source::External(Box::new(
                        v1alpha1::ExternalArtifact {
                            source_url: "https://example.com/artifact-1".to_string(),
                            source_encoding: None,
                            declared_mime: None,
                            fetched_at: MessageField::some(invalid_timestamp()),
                            content_digest: MessageField::none(),
                        },
                    ))),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidTimestamp {
            field: "artifact_metadata.external.fetched_at"
        })
    );
}

#[test]
fn validate_user_message_recorded_accepts_usage_without_cost() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
                    }],
                    model: None,
                    usage: MessageField::some(v1alpha1::TokenUsage {
                        input_tokens: Some(10),
                        output_tokens: None,
                        cache_creation_tokens: None,
                        cache_read_tokens: None,
                        cost: MessageField::none(),
                    }),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_accepts_valid_usage_currency_code() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
                    }],
                    model: None,
                    usage: MessageField::some(token_usage_with_currency("USD")),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_user_message_recorded_rejects_invalid_usage_currency_code() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::UserMessageRecorded {
                session_id: "session-1".to_string(),
                message: MessageField::some(v1alpha1::CanonicalMessage {
                    message_id: "message-1".to_string(),
                    role: buffa::EnumValue::from(v1alpha1::MessageRole::User),
                    content: vec![v1alpha1::ContentBlock {
                        kind: Some(v1alpha1::content_block::Kind::Text("hi".to_string())),
                    }],
                    model: None,
                    usage: MessageField::some(token_usage_with_currency("dollars")),
                    created_at: MessageField::some(valid_timestamp()),
                }),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidCurrencyCode {
            field: "message.usage.cost.currency_code"
        })
    );
}

#[test]
fn validate_compacted_accepts_valid_usage_currency_code() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::Compacted {
                session_id: "session-1".to_string(),
                summary_id: "summary-1".to_string(),
                summary_content: "summary".to_string(),
                covers_from: MessageField::some(session_ordinal(1)),
                covers_through: MessageField::some(session_ordinal(5)),
                trigger: buffa::EnumValue::from(v1alpha1::CompactionTrigger::Manual),
                guidance: None,
                tokens_before: None,
                tokens_after: None,
                model: None,
                usage: MessageField::some(token_usage_with_currency("USD")),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_compacted_rejects_invalid_usage_currency_code() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::Compacted {
                session_id: "session-1".to_string(),
                summary_id: "summary-1".to_string(),
                summary_content: "summary".to_string(),
                covers_from: MessageField::some(session_ordinal(1)),
                covers_through: MessageField::some(session_ordinal(5)),
                trigger: buffa::EnumValue::from(v1alpha1::CompactionTrigger::Manual),
                guidance: None,
                tokens_before: None,
                tokens_after: None,
                model: None,
                usage: MessageField::some(token_usage_with_currency("usd")),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidCurrencyCode {
            field: "usage.cost.currency_code"
        })
    );
}

#[test]
fn validate_assistant_message_failed_accepts_valid_usage_currency_code() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageFailed {
                session_id: "session-1".to_string(),
                message_id: "message-1".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::AssistantMessageFailureReason::Error),
                detail: None,
                usage: MessageField::some(token_usage_with_currency("EUR")),
            }
            .into(),
        ),
    };

    assert_eq!(validate_session_event(&event), Ok(()));
}

#[test]
fn validate_assistant_message_failed_rejects_invalid_usage_currency_code() {
    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::AssistantMessageFailed {
                session_id: "session-1".to_string(),
                message_id: "message-1".to_string(),
                reason: buffa::EnumValue::from(v1alpha1::AssistantMessageFailureReason::Error),
                detail: None,
                usage: MessageField::some(token_usage_with_currency("E1")),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::InvalidCurrencyCode {
            field: "usage.cost.currency_code"
        })
    );
}

#[test]
fn validate_checkpoint_produced_rejects_empty_checkpoint_type() {
    let mut broken_checkpoint = checkpoint();
    broken_checkpoint.checkpoint_type = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::CheckpointProduced {
                session_id: "session-1".to_string(),
                checkpoint: MessageField::some(broken_checkpoint),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "checkpoint.checkpoint_type"
        })
    );
}

#[test]
fn validate_checkpoint_produced_rejects_empty_implementation_version() {
    let mut broken_checkpoint = checkpoint();
    broken_checkpoint.implementation_version = String::new();

    let event = v1alpha1::SessionEvent {
        event: Some(
            v1alpha1::CheckpointProduced {
                session_id: "session-1".to_string(),
                checkpoint: MessageField::some(broken_checkpoint),
            }
            .into(),
        ),
    };

    assert_eq!(
        validate_session_event(&event),
        Err(SessionEventValidationError::EmptyIdentifier {
            field: "checkpoint.implementation_version"
        })
    );
}
