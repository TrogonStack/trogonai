use super::*;

#[test]
fn next_retry_delay_doubles_until_ceiling() {
    assert_eq!(next_retry_delay(Duration::from_millis(100)), Duration::from_millis(200));
    assert_eq!(
        next_retry_delay(Duration::from_millis(800)),
        Duration::from_millis(1600)
    );
    assert_eq!(next_retry_delay(MAX_RETRY_DELAY), MAX_RETRY_DELAY);
    assert_eq!(next_retry_delay(Duration::from_secs(60)), MAX_RETRY_DELAY);
}

#[test]
fn parse_http_target_accepts_http_and_https() {
    assert!(parse_http_target("https://example.com/hook").is_ok());
    assert!(parse_http_target("http://localhost:8080/hook").is_ok());
}

#[test]
fn parse_http_target_rejects_non_http_target() {
    let err = parse_http_target("subject:a2a.push.t.caller.task").unwrap_err();
    assert!(matches!(err, DispatchError::InvalidTarget(_)));
}

#[tokio::test]
async fn dispatch_returns_invalid_target_for_non_http_scheme() {
    let dispatcher = HttpPushDispatcher::new(reqwest::Client::new());
    let config = TaskPushNotificationConfig {
        url: "subject:a2a.push.t.caller.task".into(),
        id: Some("cfg-1".into()),
        task_id: String::new(),
        token: None,
        authentication: None,
        tenant: None,
    };
    let err = dispatcher
        .dispatch(
            &A2aTaskId::new("task-1").unwrap(),
            &config,
            DeliverySemantics::AtLeastOnce,
            TerminalPushTaskState::Completed,
            b"{}",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::InvalidTarget(_)));
}

#[tokio::test]
async fn dispatch_returns_invalid_authorization_when_scheme_unsupported() {
    let dispatcher = HttpPushDispatcher::new(reqwest::Client::new());
    let config = TaskPushNotificationConfig {
        url: "https://example.invalid/hook".into(),
        id: Some("cfg-1".into()),
        task_id: String::new(),
        token: None,
        authentication: Some(a2a::types::AuthenticationInfo {
            scheme: "Digest".into(),
            credentials: Some("opaque".into()),
        }),
        tenant: None,
    };
    let err = dispatcher
        .dispatch(
            &A2aTaskId::new("task-1").unwrap(),
            &config,
            DeliverySemantics::AtLeastOnce,
            TerminalPushTaskState::Completed,
            b"{}",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::InvalidAuthorization(_)));
}

#[tokio::test]
async fn dispatch_returns_prep_error_when_exactly_once_lacks_config_id() {
    let dispatcher = HttpPushDispatcher::new(reqwest::Client::new());
    let config = TaskPushNotificationConfig {
        url: "https://example.invalid/hook".into(),
        id: None,
        task_id: String::new(),
        token: None,
        authentication: None,
        tenant: None,
    };
    let err = dispatcher
        .dispatch(
            &A2aTaskId::new("task-1").unwrap(),
            &config,
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
            TerminalPushTaskState::Completed,
            b"{}",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::Prep(_)));
}

#[tokio::test]
async fn dispatch_returns_invalid_header_when_idempotency_key_carries_control_chars() {
    // PushNotificationConfigId currently accepts arbitrary non-empty
    // strings, so a config id with control characters propagates into
    // the derived idempotency key and fails reqwest's HeaderValue parse.
    let dispatcher = HttpPushDispatcher::new(reqwest::Client::new());
    let config = TaskPushNotificationConfig {
        url: "https://example.invalid/hook".into(),
        id: Some("cfg\r\n-1".into()),
        task_id: String::new(),
        token: None,
        authentication: None,
        tenant: None,
    };
    let err = dispatcher
        .dispatch(
            &A2aTaskId::new("task-1").unwrap(),
            &config,
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
            TerminalPushTaskState::Completed,
            b"{}",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::InvalidHeader(_)));
}

#[tokio::test]
async fn push_transport_retryable_classifies_a_real_connect_failure() {
    // Use port 1 (reserved) so the connect attempt fails immediately.
    // The classifier must mark the resulting error as retryable so the
    // dispatcher backs off + retries rather than treating it as terminal.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let err = client.get("http://127.0.0.1:1/hook").send().await.unwrap_err();
    assert!(
        push_transport_retryable(&err),
        "connect refusal to a reserved port must classify as retryable"
    );
}
