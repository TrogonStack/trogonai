use super::*;

#[test]
fn new_carries_code_and_message() {
    let e = A2aError::new(-32000, "boom");
    assert_eq!(e.code, -32000);
    assert_eq!(e.message, "boom");
}

#[test]
fn helpers_use_their_spec_codes() {
    assert_eq!(A2aError::task_not_found("x").code, TASK_NOT_FOUND);
    assert_eq!(A2aError::task_not_cancelable("x").code, TASK_NOT_CANCELABLE);
    assert_eq!(
        A2aError::push_notification_not_supported("x").code,
        PUSH_NOTIFICATION_NOT_SUPPORTED
    );
    assert_eq!(A2aError::unsupported_operation("x").code, UNSUPPORTED_OPERATION);
    assert_eq!(
        A2aError::content_type_not_supported("x").code,
        CONTENT_TYPE_NOT_SUPPORTED
    );
    assert_eq!(A2aError::invalid_agent_response("x").code, INVALID_AGENT_RESPONSE);
    assert_eq!(A2aError::agent_unavailable("x").code, AGENT_UNAVAILABLE);
    assert_eq!(
        A2aError::extended_agent_card_not_configured("x").code,
        EXTENDED_AGENT_CARD_NOT_CONFIGURED
    );
    assert_eq!(
        A2aError::extension_support_required("x").code,
        EXTENSION_SUPPORT_REQUIRED
    );
    assert_eq!(A2aError::version_not_supported("x").code, VERSION_NOT_SUPPORTED);
    assert_eq!(A2aError::internal("x").code, -32603);
}

#[test]
fn display_includes_code_and_message() {
    assert_eq!(
        format!("{}", A2aError::task_not_found("missing")),
        format!("[{TASK_NOT_FOUND}] missing")
    );
}

#[test]
fn implements_std_error() {
    let e: Box<dyn std::error::Error> = Box::new(A2aError::internal("oops"));
    assert!(e.to_string().contains("oops"));
}
