use super::*;

#[test]
fn default_prefix_is_a2a() {
    assert_eq!(DEFAULT_A2A_PREFIX, "a2a");
}

#[test]
fn default_timeouts_are_positive() {
    assert!(DEFAULT_OPERATION_TIMEOUT > Duration::ZERO);
    assert!(DEFAULT_TASK_TIMEOUT > Duration::ZERO);
    const _: () = assert!(DEFAULT_CONNECT_TIMEOUT_SECS > 0);
}

#[test]
fn task_timeout_exceeds_operation_timeout() {
    assert!(DEFAULT_TASK_TIMEOUT > DEFAULT_OPERATION_TIMEOUT);
}

#[test]
fn env_var_names_are_namespaced() {
    for name in [
        ENV_A2A_PREFIX,
        ENV_OPERATION_TIMEOUT_SECS,
        ENV_TASK_TIMEOUT_SECS,
        ENV_CONNECT_TIMEOUT_SECS,
        ENV_PUSH_DLQ_CALLER_SEGMENT,
        ENV_PUSH_DLQ_DEDUP_LRU_SIZE,
        ENV_PUSH_DLQ_DEDUP_WINDOW_SECS,
        ENV_MAX_CONCURRENT_CLIENT_TASKS,
        ENV_EVENTS_MAX_AGE_SECS,
    ] {
        assert!(name.starts_with("A2A_"), "{name} not A2A-namespaced");
    }
}

#[test]
fn headers_use_x_prefix() {
    assert!(REQ_ID_HEADER.starts_with("X-"));
    assert!(GATEWAY_CALLER_ID_HEADER.starts_with("X-"));
    assert!(GATEWAY_PRINCIPAL_HEADER.starts_with("X-"));
    assert!(
        GATEWAY_CALLER_ID_HTTP
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "HTTP header literals should be lowercase with digits/hyphen only for axum pairing"
    );
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn http_push_max_attempts_is_positive() {
    assert!(crate::constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS > 0);
}
