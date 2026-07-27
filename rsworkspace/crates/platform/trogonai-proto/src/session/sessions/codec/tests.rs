use buffa::{Message as _, MessageField};
use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

use super::*;

fn timestamp() -> buffa_types::google::protobuf::Timestamp {
    buffa_types::google::protobuf::Timestamp::from_unix(1_451_600_400, 0)
}

fn digest() -> v1alpha1::Digest {
    v1alpha1::Digest {
        algorithm: "sha256".to_string(),
        value: vec![0u8; 32],
    }
}

fn session_ordinal(value: u64) -> v1alpha1::SessionOrdinal {
    v1alpha1::SessionOrdinal { value }
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

fn canonical_message(role: v1alpha1::MessageRole) -> v1alpha1::CanonicalMessage {
    v1alpha1::CanonicalMessage {
        message_id: "message-1".to_string(),
        role: buffa::EnumValue::from(role),
        content: vec![v1alpha1::ContentBlock {
            kind: Some(v1alpha1::content_block::Kind::Text("hello".to_string())),
        }],
        model: None,
        usage: MessageField::none(),
        created_at: MessageField::some(timestamp()),
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

fn session_started() -> v1alpha1::SessionStarted {
    v1alpha1::SessionStarted {
        session_id: "session-1".to_string(),
        execution_plan: MessageField::some(v1alpha1::StoredSessionExecutionPlan {
            plan_bytes: b"plan".to_vec(),
            plan_digest: MessageField::some(digest()),
        }),
    }
}

fn session_closed() -> v1alpha1::SessionClosed {
    v1alpha1::SessionClosed {
        session_id: "session-1".to_string(),
        result_ref: MessageField::some(artifact_ref()),
    }
}

fn session_cancelled() -> v1alpha1::SessionCancelled {
    v1alpha1::SessionCancelled {
        session_id: "session-1".to_string(),
        reason: buffa::EnumValue::from(v1alpha1::SessionCancellationReason::UserRequested),
        detail: None,
    }
}

fn session_failed() -> v1alpha1::SessionFailed {
    v1alpha1::SessionFailed {
        session_id: "session-1".to_string(),
        detail: Some("boom".to_string()),
        reason: buffa::EnumValue::from(v1alpha1::SessionFailureReason::ExecutionError),
    }
}

fn session_hidden() -> v1alpha1::SessionHidden {
    v1alpha1::SessionHidden {
        session_id: "session-1".to_string(),
        reason: buffa::EnumValue::from(v1alpha1::SessionHiddenReason::UserRequested),
    }
}

fn session_forked() -> v1alpha1::SessionForked {
    v1alpha1::SessionForked {
        session_id: "session-1".to_string(),
        source_session_id: "session-0".to_string(),
        context_prefix_boundary: MessageField::some(session_ordinal(3)),
        reason: buffa::EnumValue::from(v1alpha1::ForkReason::ManualBranch),
    }
}

fn session_rewound() -> v1alpha1::SessionRewound {
    v1alpha1::SessionRewound {
        session_id: "session-1".to_string(),
        keep_through: MessageField::some(session_ordinal(2)),
        reason: buffa::EnumValue::from(v1alpha1::RewindReason::Manual),
    }
}

fn compacted() -> v1alpha1::Compacted {
    v1alpha1::Compacted {
        session_id: "session-1".to_string(),
        summary_id: "summary-1".to_string(),
        summary_content: "summary".to_string(),
        covers_from: MessageField::some(session_ordinal(1)),
        covers_through: MessageField::some(session_ordinal(5)),
        trigger: buffa::EnumValue::from(v1alpha1::CompactionTrigger::Manual),
        guidance: None,
        tokens_before: Some(100),
        tokens_after: Some(10),
        model: Some("model".to_string()),
        usage: MessageField::none(),
    }
}

fn user_message_recorded() -> v1alpha1::UserMessageRecorded {
    v1alpha1::UserMessageRecorded {
        session_id: "session-1".to_string(),
        message: MessageField::some(canonical_message(v1alpha1::MessageRole::User)),
    }
}

fn assistant_message_started() -> v1alpha1::AssistantMessageStarted {
    v1alpha1::AssistantMessageStarted {
        session_id: "session-1".to_string(),
        message_id: "message-1".to_string(),
        model: "model".to_string(),
    }
}

fn assistant_message_completed() -> v1alpha1::AssistantMessageCompleted {
    v1alpha1::AssistantMessageCompleted {
        session_id: "session-1".to_string(),
        message: MessageField::some(canonical_message(v1alpha1::MessageRole::Assistant)),
        finish_reason: buffa::EnumValue::from(v1alpha1::FinishReason::EndTurn),
        matched_stop_sequence: None,
    }
}

fn assistant_message_failed() -> v1alpha1::AssistantMessageFailed {
    v1alpha1::AssistantMessageFailed {
        session_id: "session-1".to_string(),
        message_id: "message-1".to_string(),
        reason: buffa::EnumValue::from(v1alpha1::AssistantMessageFailureReason::Error),
        detail: None,
        usage: MessageField::none(),
    }
}

fn tool_call_requested() -> v1alpha1::ToolCallRequested {
    v1alpha1::ToolCallRequested {
        session_id: "session-1".to_string(),
        tool_call_id: "tool-call-1".to_string(),
        tool_execution_id: "tool-exec-1".to_string(),
        tool_name: "search".to_string(),
        input_json: "{}".to_string(),
        parent_tool_use_id: None,
        operation_id: None,
    }
}

fn tool_call_approved() -> v1alpha1::ToolCallApproved {
    v1alpha1::ToolCallApproved {
        session_id: "session-1".to_string(),
        tool_call_id: "tool-call-1".to_string(),
        tool_execution_id: "tool-exec-1".to_string(),
        approved_by: "user-1".to_string(),
    }
}

fn tool_call_denied() -> v1alpha1::ToolCallDenied {
    v1alpha1::ToolCallDenied {
        session_id: "session-1".to_string(),
        tool_call_id: "tool-call-1".to_string(),
        tool_execution_id: "tool-exec-1".to_string(),
        denied_by: "user-1".to_string(),
        reason: None,
    }
}

fn tool_call_started() -> v1alpha1::ToolCallStarted {
    v1alpha1::ToolCallStarted {
        session_id: "session-1".to_string(),
        tool_call_id: "tool-call-1".to_string(),
        tool_execution_id: "tool-exec-1".to_string(),
    }
}

fn tool_call_completed() -> v1alpha1::ToolCallCompleted {
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
}

fn tool_call_failed() -> v1alpha1::ToolCallFailed {
    v1alpha1::ToolCallFailed {
        session_id: "session-1".to_string(),
        tool_call_id: "tool-call-1".to_string(),
        tool_execution_id: "tool-exec-1".to_string(),
        error: "boom".to_string(),
        reason: buffa::EnumValue::from(v1alpha1::ToolCallFailureReason::Error),
    }
}

fn artifact_recorded() -> v1alpha1::ArtifactRecorded {
    v1alpha1::ArtifactRecorded {
        session_id: "session-1".to_string(),
        artifact: MessageField::some(v1alpha1::ArtifactMetadata {
            artifact_id: "artifact-1".to_string(),
            preview: None,
            truncated: None,
            created_at: MessageField::some(timestamp()),
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
}

fn file_changed() -> v1alpha1::FileChanged {
    v1alpha1::FileChanged {
        session_id: "session-1".to_string(),
        path: "src/main.rs".to_string(),
        change_kind: buffa::EnumValue::from(v1alpha1::FileChangeKind::Modified),
        previous_path: None,
        before_ref: MessageField::some(artifact_ref()),
        after_ref: MessageField::some(artifact_ref()),
    }
}

fn execution_attempt_started() -> v1alpha1::ExecutionAttemptStarted {
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
        started_at: MessageField::some(timestamp()),
    }
}

fn execution_attempt_ready() -> v1alpha1::ExecutionAttemptReady {
    v1alpha1::ExecutionAttemptReady {
        session_id: "session-1".to_string(),
        execution_attempt_id: "attempt-1".to_string(),
        ready_attestation_ref: "ready-ref".to_string(),
        ready_attestation_digest: MessageField::some(digest()),
        ready_at: MessageField::some(timestamp()),
    }
}

fn execution_attempt_ended() -> v1alpha1::ExecutionAttemptEnded {
    v1alpha1::ExecutionAttemptEnded {
        session_id: "session-1".to_string(),
        execution_attempt_id: "attempt-1".to_string(),
        outcome: buffa::EnumValue::from(v1alpha1::AttemptOutcome::Failed),
        detail: None,
        ended_at: MessageField::some(timestamp()),
    }
}

fn checkpoint_produced() -> v1alpha1::CheckpointProduced {
    v1alpha1::CheckpointProduced {
        session_id: "session-1".to_string(),
        checkpoint: MessageField::some(checkpoint()),
    }
}

fn delegation_dispatched() -> v1alpha1::DelegationDispatched {
    v1alpha1::DelegationDispatched {
        session_id: "session-1".to_string(),
        operation_id: "operation-1".to_string(),
        child_session_id: "session-2".to_string(),
        cascade_policy: buffa::EnumValue::from(v1alpha1::CascadePolicy::CascadeOnParentTerminal),
    }
}

fn parent_linked() -> v1alpha1::ParentLinked {
    v1alpha1::ParentLinked {
        session_id: "session-1".to_string(),
        parent_session_id: "session-0".to_string(),
        parent_dispatched_at: MessageField::some(session_ordinal(1)),
        cascade_policy: buffa::EnumValue::from(v1alpha1::CascadePolicy::CascadeOnParentTerminal),
        operation_id: "operation-1".to_string(),
    }
}

fn parent_terminated() -> v1alpha1::ParentTerminated {
    v1alpha1::ParentTerminated {
        session_id: "session-1".to_string(),
        parent_session_id: "session-0".to_string(),
        cause: buffa::EnumValue::from(v1alpha1::ParentTerminalCause::Closed),
        triggering_event_id: "event-1".to_string(),
    }
}

fn delegation_detached() -> v1alpha1::DelegationDetached {
    v1alpha1::DelegationDetached {
        session_id: "session-1".to_string(),
        child_session_id: "session-2".to_string(),
        reason: None,
        detach_operation_id: "operation-1".to_string(),
    }
}

fn parent_history_invalidated() -> v1alpha1::ParentHistoryInvalidated {
    v1alpha1::ParentHistoryInvalidated {
        session_id: "session-1".to_string(),
        parent_session_id: "session-0".to_string(),
        parent_keep_through: MessageField::some(session_ordinal(4)),
        triggering_event_id: "event-1".to_string(),
    }
}

fn parent_detached() -> v1alpha1::ParentDetached {
    v1alpha1::ParentDetached {
        session_id: "session-1".to_string(),
        parent_session_id: "session-0".to_string(),
        detach_operation_id: "operation-1".to_string(),
    }
}

fn external_delegation_dispatched() -> v1alpha1::ExternalDelegationDispatched {
    v1alpha1::ExternalDelegationDispatched {
        session_id: "session-1".to_string(),
        operation_id: "operation-1".to_string(),
        delegate_reference: "delegate-ref".to_string(),
        authenticated_remote_subject: "subject-1".to_string(),
        authorization_reference: "authz-ref".to_string(),
        request_digest: MessageField::some(digest()),
        correlation_id: "correlation-1".to_string(),
    }
}

fn operation_reserved() -> v1alpha1::OperationReserved {
    v1alpha1::OperationReserved {
        session_id: "session-1".to_string(),
        operation_id: "operation-1".to_string(),
        request_digest: MessageField::some(digest()),
        operation_kind: buffa::EnumValue::from(v1alpha1::OperationKind::Tool),
    }
}

fn operation_outcome_recorded() -> v1alpha1::OperationOutcomeRecorded {
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
}

fn operation_cancellation_requested() -> v1alpha1::OperationCancellationRequested {
    v1alpha1::OperationCancellationRequested {
        session_id: "session-1".to_string(),
        operation_id: "operation-1".to_string(),
        reason: None,
    }
}

fn artifact_erased() -> v1alpha1::ArtifactErased {
    v1alpha1::ArtifactErased {
        session_id: "session-1".to_string(),
        artifact_id: "artifact-1".to_string(),
        reason: None,
    }
}

fn redaction_applied() -> v1alpha1::RedactionApplied {
    v1alpha1::RedactionApplied {
        session_id: "session-1".to_string(),
        redacted_event_ids: vec!["event-1".to_string()],
        reason: None,
    }
}

fn system_notice_recorded() -> v1alpha1::SystemNoticeRecorded {
    v1alpha1::SystemNoticeRecorded {
        session_id: "session-1".to_string(),
        level: buffa::EnumValue::from(v1alpha1::NoticeLevel::Info),
        text: "notice".to_string(),
        tool_call_id: None,
    }
}

fn todo_updated() -> v1alpha1::TodoUpdated {
    v1alpha1::TodoUpdated {
        session_id: "session-1".to_string(),
        items: vec![v1alpha1::TodoItem {
            id: "todo-1".to_string(),
            content: "write tests".to_string(),
            status: buffa::EnumValue::from(v1alpha1::TodoStatus::Pending),
        }],
        revision: 1,
    }
}

fn session_renamed() -> v1alpha1::SessionRenamed {
    v1alpha1::SessionRenamed {
        session_id: "session-1".to_string(),
        display_name: "New title".to_string(),
    }
}

fn session_archived() -> v1alpha1::SessionArchived {
    v1alpha1::SessionArchived {
        session_id: "session-1".to_string(),
    }
}

fn session_unarchived() -> v1alpha1::SessionUnarchived {
    v1alpha1::SessionUnarchived {
        session_id: "session-1".to_string(),
    }
}

fn all_session_events() -> Vec<v1alpha1::SessionEvent> {
    vec![
        v1alpha1::SessionEvent {
            event: Some(session_started().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_closed().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_cancelled().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_failed().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_hidden().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_forked().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_rewound().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(compacted().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(user_message_recorded().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(assistant_message_started().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(assistant_message_completed().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(assistant_message_failed().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(tool_call_requested().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(tool_call_approved().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(tool_call_denied().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(tool_call_started().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(tool_call_completed().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(tool_call_failed().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(artifact_recorded().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(file_changed().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(execution_attempt_started().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(execution_attempt_ready().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(execution_attempt_ended().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(checkpoint_produced().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(delegation_dispatched().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(parent_linked().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(parent_terminated().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(delegation_detached().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(parent_history_invalidated().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(parent_detached().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(external_delegation_dispatched().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(operation_reserved().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(operation_outcome_recorded().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(operation_cancellation_requested().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(artifact_erased().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(redaction_applied().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(system_notice_recorded().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(todo_updated().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_renamed().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_archived().into()),
        },
        v1alpha1::SessionEvent {
            event: Some(session_unarchived().into()),
        },
    ]
}

/// Decodes `encoded` back through the concrete inner type named by `event`'s variant,
/// asserts it round-trips to the same value, and returns the variant's generated full name.
fn assert_variant_round_trips(event: &v1alpha1::SessionEvent, encoded: &[u8]) -> &'static str {
    match event.event.as_ref().unwrap() {
        SessionEventCase::SessionStarted(inner) => {
            assert_eq!(v1alpha1::SessionStarted::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionStarted as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionClosed(inner) => {
            assert_eq!(v1alpha1::SessionClosed::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionClosed as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionCancelled(inner) => {
            assert_eq!(v1alpha1::SessionCancelled::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionCancelled as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionFailed(inner) => {
            assert_eq!(v1alpha1::SessionFailed::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionFailed as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionHidden(inner) => {
            assert_eq!(v1alpha1::SessionHidden::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionHidden as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionForked(inner) => {
            assert_eq!(v1alpha1::SessionForked::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionForked as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionRewound(inner) => {
            assert_eq!(v1alpha1::SessionRewound::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionRewound as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::Compacted(inner) => {
            assert_eq!(v1alpha1::Compacted::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::Compacted as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::UserMessageRecorded(inner) => {
            assert_eq!(
                v1alpha1::UserMessageRecorded::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::UserMessageRecorded as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::AssistantMessageStarted(inner) => {
            assert_eq!(
                v1alpha1::AssistantMessageStarted::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::AssistantMessageStarted as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::AssistantMessageCompleted(inner) => {
            assert_eq!(
                v1alpha1::AssistantMessageCompleted::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::AssistantMessageCompleted as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::AssistantMessageFailed(inner) => {
            assert_eq!(
                v1alpha1::AssistantMessageFailed::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::AssistantMessageFailed as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ToolCallRequested(inner) => {
            assert_eq!(
                v1alpha1::ToolCallRequested::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::ToolCallRequested as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ToolCallApproved(inner) => {
            assert_eq!(v1alpha1::ToolCallApproved::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ToolCallApproved as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ToolCallDenied(inner) => {
            assert_eq!(v1alpha1::ToolCallDenied::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ToolCallDenied as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ToolCallStarted(inner) => {
            assert_eq!(v1alpha1::ToolCallStarted::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ToolCallStarted as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ToolCallCompleted(inner) => {
            assert_eq!(
                v1alpha1::ToolCallCompleted::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::ToolCallCompleted as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ToolCallFailed(inner) => {
            assert_eq!(v1alpha1::ToolCallFailed::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ToolCallFailed as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ArtifactRecorded(inner) => {
            assert_eq!(v1alpha1::ArtifactRecorded::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ArtifactRecorded as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::FileChanged(inner) => {
            assert_eq!(v1alpha1::FileChanged::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::FileChanged as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ExecutionAttemptStarted(inner) => {
            assert_eq!(
                v1alpha1::ExecutionAttemptStarted::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::ExecutionAttemptStarted as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ExecutionAttemptReady(inner) => {
            assert_eq!(
                v1alpha1::ExecutionAttemptReady::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::ExecutionAttemptReady as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ExecutionAttemptEnded(inner) => {
            assert_eq!(
                v1alpha1::ExecutionAttemptEnded::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::ExecutionAttemptEnded as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::CheckpointProduced(inner) => {
            assert_eq!(
                v1alpha1::CheckpointProduced::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::CheckpointProduced as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::DelegationDispatched(inner) => {
            assert_eq!(
                v1alpha1::DelegationDispatched::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::DelegationDispatched as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ParentLinked(inner) => {
            assert_eq!(v1alpha1::ParentLinked::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ParentLinked as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ParentTerminated(inner) => {
            assert_eq!(v1alpha1::ParentTerminated::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ParentTerminated as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::DelegationDetached(inner) => {
            assert_eq!(
                v1alpha1::DelegationDetached::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::DelegationDetached as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ParentHistoryInvalidated(inner) => {
            assert_eq!(
                v1alpha1::ParentHistoryInvalidated::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::ParentHistoryInvalidated as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ParentDetached(inner) => {
            assert_eq!(v1alpha1::ParentDetached::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ParentDetached as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ExternalDelegationDispatched(inner) => {
            assert_eq!(
                v1alpha1::ExternalDelegationDispatched::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::ExternalDelegationDispatched as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::OperationReserved(inner) => {
            assert_eq!(
                v1alpha1::OperationReserved::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::OperationReserved as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::OperationOutcomeRecorded(inner) => {
            assert_eq!(
                v1alpha1::OperationOutcomeRecorded::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::OperationOutcomeRecorded as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::OperationCancellationRequested(inner) => {
            assert_eq!(
                v1alpha1::OperationCancellationRequested::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::OperationCancellationRequested as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::ArtifactErased(inner) => {
            assert_eq!(v1alpha1::ArtifactErased::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::ArtifactErased as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::RedactionApplied(inner) => {
            assert_eq!(v1alpha1::RedactionApplied::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::RedactionApplied as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SystemNoticeRecorded(inner) => {
            assert_eq!(
                v1alpha1::SystemNoticeRecorded::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::SystemNoticeRecorded as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::TodoUpdated(inner) => {
            assert_eq!(v1alpha1::TodoUpdated::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::TodoUpdated as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionRenamed(inner) => {
            assert_eq!(v1alpha1::SessionRenamed::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionRenamed as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionArchived(inner) => {
            assert_eq!(v1alpha1::SessionArchived::decode_from_slice(encoded).unwrap(), **inner);
            <v1alpha1::SessionArchived as buffa::MessageName>::FULL_NAME
        }
        SessionEventCase::SessionUnarchived(inner) => {
            assert_eq!(
                v1alpha1::SessionUnarchived::decode_from_slice(encoded).unwrap(),
                **inner
            );
            <v1alpha1::SessionUnarchived as buffa::MessageName>::FULL_NAME
        }
    }
}

#[test]
fn event_encode_writes_inner_event_payload() {
    let inner = session_started();
    let event = v1alpha1::SessionEvent {
        event: Some(inner.clone().into()),
    };

    let encoded = EventEncode::encode(&event).unwrap();

    assert_eq!(v1alpha1::SessionStarted::decode_from_slice(&encoded).unwrap(), inner);
}

#[test]
fn event_encode_rejects_missing_event_case() {
    let event = v1alpha1::SessionEvent { event: None };

    assert!(matches!(
        EventEncode::encode(&event),
        Err(SessionEventPayloadError::MissingEvent)
    ));
}

#[test]
fn event_encode_writes_all_lifecycle_event_payloads() {
    for event in all_session_events() {
        let encoded = EventEncode::encode(&event).unwrap();
        assert_variant_round_trips(&event, &encoded);
    }
}

#[test]
fn event_decode_dispatches_by_generated_full_name() {
    let inner = session_started();
    let encoded = inner.encode_to_vec();

    let decoded = <v1alpha1::SessionEvent as EventDecode>::decode(EventData::new(
        <v1alpha1::SessionStarted as buffa::MessageName>::FULL_NAME,
        &encoded,
    ))
    .unwrap();

    let decoded = decoded.into_decoded().unwrap();
    assert!(matches!(decoded.event, Some(SessionEventCase::SessionStarted(_))));
}

#[test]
fn event_decode_dispatches_all_lifecycle_event_types() {
    for event in all_session_events() {
        let encoded = EventEncode::encode(&event).unwrap();
        let full_name = assert_variant_round_trips(&event, &encoded);

        let decoded = <v1alpha1::SessionEvent as EventDecode>::decode(EventData::new(full_name, &encoded))
            .unwrap()
            .into_decoded()
            .unwrap();

        assert_eq!(
            std::mem::discriminant(decoded.event.as_ref().unwrap()),
            std::mem::discriminant(event.event.as_ref().unwrap())
        );
    }
}

#[test]
fn event_decode_skips_unknown_event_type() {
    assert!(matches!(
        <v1alpha1::SessionEvent as EventDecode>::decode(EventData::new(
            "trogonai.session.sessions.v1alpha1.Unknown",
            &[]
        )),
        Ok(EventDecodeOutcome::Skipped)
    ));
}

#[test]
fn event_decode_preserves_payload_decode_errors() {
    assert!(matches!(
        <v1alpha1::SessionEvent as EventDecode>::decode(EventData::new(
            <v1alpha1::SessionArchived as buffa::MessageName>::FULL_NAME,
            b"\0"
        )),
        Err(SessionEventPayloadError::Decode(_))
    ));
}

#[test]
fn event_type_returns_inner_event_full_name() {
    let event = v1alpha1::SessionEvent {
        event: Some(session_archived().into()),
    };

    assert_eq!(
        event.event_type().unwrap(),
        <v1alpha1::SessionArchived as buffa::MessageName>::FULL_NAME
    );
}

#[test]
fn event_type_returns_all_lifecycle_event_full_names() {
    for event in all_session_events() {
        let encoded = EventEncode::encode(&event).unwrap();
        let full_name = assert_variant_round_trips(&event, &encoded);

        assert_eq!(event.event_type().unwrap(), full_name);
    }
}

#[test]
fn event_type_rejects_missing_event_case() {
    let event = v1alpha1::SessionEvent { event: None };

    assert!(matches!(
        event.event_type(),
        Err(SessionEventPayloadError::MissingEvent)
    ));
}
