use super::*;

#[test]
fn new_trims_and_accepts_non_empty() {
    let id = PushNotificationConfigId::new("  cfg-1 ").unwrap();
    assert_eq!(id.as_str(), "cfg-1");
    assert_eq!(id.to_string(), "cfg-1");
}

#[test]
fn new_rejects_empty_and_blank() {
    assert_eq!(
        PushNotificationConfigId::new(""),
        Err(PushNotificationConfigIdError::Empty)
    );
    assert_eq!(
        PushNotificationConfigId::new("   "),
        Err(PushNotificationConfigIdError::Empty)
    );
}

#[test]
fn new_accepts_colon_now_that_idempotency_key_is_length_prefixed() {
    let id = PushNotificationConfigId::new("cfg:1").unwrap();
    assert_eq!(id.as_str(), "cfg:1");
}

#[test]
fn error_display_and_debug_render_distinct_messages() {
    let empty = PushNotificationConfigIdError::Empty;
    assert_eq!(empty.to_string(), "push notification config id cannot be empty");
    assert!(format!("{empty:?}").contains("Empty"));
}

#[test]
fn id_debug_reveals_inner_value() {
    let id = PushNotificationConfigId::new("cfg-2").unwrap();
    assert!(format!("{id:?}").contains("cfg-2"));
}
