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
        created_at: MessageField::none(),
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
                    created_at: MessageField::none(),
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
                    created_at: MessageField::none(),
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
                    created_at: MessageField::none(),
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
                started_at: MessageField::none(),
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
                started_at: MessageField::none(),
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
