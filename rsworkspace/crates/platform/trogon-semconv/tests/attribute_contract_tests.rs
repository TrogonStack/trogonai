use std::fmt::Debug;

use trogon_semconv::attribute::{
    DecisionOutcome, ErrorType, GuestPhase, McpNatsDirection, MessagingOperationName, MessagingOperationType,
    MessagingSystem, Method, Operation, Outcome, Reason, SnapshotOutcome, TrapClassification, WritePrecondition,
};

fn assert_wire_values<E: Debug>(cases: &[(E, &str)], encode: fn(&E) -> &'static str) {
    for (value, expected) in cases {
        assert_eq!(encode(value), *expected, "wire value for {value:?}");
    }
}

#[test]
fn messaging_attributes_match_registered_wire_values() {
    assert_wire_values(
        &[(McpNatsDirection::Send, "send"), (McpNatsDirection::Receive, "receive")],
        McpNatsDirection::as_str,
    );
    assert_wire_values(
        &[
            (MessagingOperationName::Request, "request"),
            (MessagingOperationName::Publish, "publish"),
        ],
        MessagingOperationName::as_str,
    );
    assert_wire_values(
        &[(MessagingOperationType::Send, "send")],
        MessagingOperationType::as_str,
    );
    assert_wire_values(&[(MessagingSystem::Nats, "nats")], MessagingSystem::as_str);
    assert_wire_values(
        &[
            (ErrorType::Deserialize, "deserialize"),
            (ErrorType::FlushOperation, "flush_operation"),
            (ErrorType::PublishOperation, "publish_operation"),
            (ErrorType::Request, "request"),
            (ErrorType::Serialize, "serialize"),
            (ErrorType::Timeout, "timeout"),
        ],
        ErrorType::as_str,
    );
}

#[test]
fn acp_method_names_keep_registered_request_and_notification_spelling() {
    assert_wire_values(
        &[
            (Method::Initialize, "initialize"),
            (Method::Authenticate, "authenticate"),
            (Method::NewSession, "new_session"),
            (Method::LoadSession, "load_session"),
            (Method::ForkSession, "fork_session"),
            (Method::CloseSession, "close_session"),
            (Method::ResumeSession, "resume_session"),
            (Method::Prompt, "prompt"),
            (Method::Cancel, "cancel"),
            (Method::ListSessions, "list_sessions"),
            (Method::SetSessionModel, "set_session_model"),
            (Method::SetSessionMode, "set_session_mode"),
            (Method::SetSessionConfigOption, "set_session_config_option"),
            (Method::ExtMethod, "ext_method"),
            (Method::ExtNotification, "ext_notification"),
            (Method::Logout, "logout"),
        ],
        Method::as_str,
    );
}

#[test]
fn acp_error_operations_and_reasons_remain_distinct_on_the_wire() {
    assert_wire_values(
        &[
            (Operation::Prompt, "prompt"),
            (Operation::Cancel, "cancel"),
            (Operation::ExtMethod, "ext_method"),
            (Operation::ExtNotification, "ext_notification"),
            (Operation::SessionValidate, "session_validate"),
            (Operation::SessionReady, "session_ready"),
            (Operation::Client, "client"),
        ],
        Operation::as_str,
    );
    assert_wire_values(
        &[
            (Reason::InvalidSessionId, "invalid_session_id"),
            (Reason::InvalidMethodName, "invalid_method_name"),
            (Reason::CancelPublishFailed, "cancel_publish_failed"),
            (Reason::ExtNotificationPublishFailed, "ext_notification_publish_failed"),
            (Reason::NotificationStreamClosed, "notification_stream_closed"),
            (Reason::NotificationConsumerError, "notification_consumer_error"),
            (Reason::BadResponsePayload, "bad_response_payload"),
            (Reason::ResponseConsumerError, "response_consumer_error"),
            (Reason::ResponseStreamClosed, "response_stream_closed"),
            (Reason::PromptTimeout, "prompt_timeout"),
            (Reason::SessionReadyPublishFailed, "session_ready_publish_failed"),
            (Reason::ClientBackpressureRejected, "client_backpressure_rejected"),
        ],
        Reason::as_str,
    );
}

#[test]
fn scheduler_outcomes_preserve_reconciliation_result_labels() {
    assert_wire_values(
        &[
            (Outcome::Published, "published"),
            (Outcome::Purged, "purged"),
            (Outcome::StoredPaused, "stored_paused"),
            (Outcome::Unsupported, "unsupported"),
            (Outcome::Expired, "expired"),
            (Outcome::ResumedExpired, "resumed_expired"),
            (Outcome::DuplicateStale, "duplicate_stale"),
            (Outcome::SkippedForeign, "skipped_foreign"),
            (Outcome::DurableFailure, "durable_failure"),
        ],
        Outcome::as_str,
    );
}

#[test]
fn decider_outcomes_preserve_admission_domain_and_execution_distinctions() {
    assert_wire_values(
        &[
            (DecisionOutcome::Decided, "decided"),
            (DecisionOutcome::Rejected, "rejected"),
            (DecisionOutcome::Faulted, "faulted"),
            (DecisionOutcome::Shed, "shed"),
            (DecisionOutcome::Denied, "denied"),
        ],
        DecisionOutcome::as_str,
    );
    assert_wire_values(
        &[
            (SnapshotOutcome::Hit, "hit"),
            (SnapshotOutcome::Miss, "miss"),
            (SnapshotOutcome::DiscardedReadFailure, "discarded_read_failure"),
            (SnapshotOutcome::DiscardedAheadOfStream, "discarded_ahead_of_stream"),
            (SnapshotOutcome::Failed, "failed"),
        ],
        SnapshotOutcome::as_str,
    );
    assert_wire_values(
        &[
            (WritePrecondition::Any, "any"),
            (WritePrecondition::StreamExists, "stream_exists"),
            (WritePrecondition::NoStream, "no_stream"),
            (WritePrecondition::At, "at"),
        ],
        WritePrecondition::as_str,
    );
}

#[test]
fn guest_lifecycle_and_traps_preserve_registered_phase_labels() {
    assert_wire_values(
        &[
            (GuestPhase::Instantiate, "instantiate"),
            (GuestPhase::Replay, "replay"),
            (GuestPhase::Decide, "decide"),
            (GuestPhase::Snapshot, "snapshot"),
            (GuestPhase::Drop, "drop"),
        ],
        GuestPhase::as_str,
    );
    assert_wire_values(
        &[
            (TrapClassification::DeadlineExceeded, "deadline_exceeded"),
            (TrapClassification::Trap, "trap"),
        ],
        TrapClassification::as_str,
    );
}
