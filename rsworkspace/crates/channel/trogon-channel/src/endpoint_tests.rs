use super::*;

#[test]
fn endpoint_accepts_negative_telegram_chat_ids() {
    let e = Endpoint::new("telegram", "mybot", "-1001234567890").expect("valid");
    assert_eq!(e.kv_key(), "telegram.mybot.-1001234567890");
}

#[test]
fn endpoint_rejects_unsafe_tokens() {
    assert_eq!(
        Endpoint::new("telegram", "my bot", "1").unwrap_err(),
        EndpointError::InvalidCharacter(' ')
    );
    assert_eq!(Endpoint::new("", "mybot", "1").unwrap_err(), EndpointError::Empty);
    assert_eq!(
        Endpoint::new("telegram", "mybot", "a.b").unwrap_err(),
        EndpointError::InvalidCharacter('.')
    );
}

#[test]
fn endpoint_accessors_each_return_their_own_token() {
    let e = Endpoint::new("telegram", "mybot", "42").expect("valid");
    assert_eq!(e.channel(), "telegram");
    assert_eq!(e.account(), "mybot");
    assert_eq!(e.peer(), "42");
}

/// Display must go through kv_key(), not a field dump, so it is a valid
/// composite key wherever an endpoint is formatted as a string.
#[test]
fn endpoint_display_renders_the_dotted_composite_key() {
    let e = Endpoint::new("telegram", "mybot", "42").expect("valid");
    assert_eq!(e.to_string(), "telegram.mybot.42");
    assert_eq!(e.to_string(), e.kv_key());
}

#[test]
fn endpoint_deserialize_rejects_an_unsafe_token() {
    let err = serde_json::from_value::<Endpoint>(serde_json::json!({
        "channel": "telegram",
        "account": "my bot",
        "peer": "1",
    }))
    .expect_err("space is unsafe");
    assert!(err.to_string().contains("invalid character"), "{err}");
}

#[test]
fn principal_id_rejects_an_empty_id() {
    let err = PrincipalId::new("").unwrap_err();
    assert_eq!(err, EndpointError::Empty);
}

/// A `.` is the interesting rejection case: tokens are joined with `.` into
/// the composite key, so allowing it in a principal id would make the key
/// ambiguous to split back apart.
#[test]
fn principal_id_rejects_a_dot() {
    let err = PrincipalId::new("abc.def").unwrap_err();
    assert_eq!(err, EndpointError::InvalidCharacter('.'));
}

#[test]
fn principal_id_as_str_returns_the_constructed_id() {
    let id = PrincipalId::new("user-42").expect("valid");
    assert_eq!(id.as_str(), "user-42");
}

#[test]
fn principal_id_display_renders_the_bare_id() {
    let id = PrincipalId::new("user-42").expect("valid");
    assert_eq!(id.to_string(), "user-42");
}

#[test]
fn principal_id_deserialize_rejects_an_unsafe_token() {
    let err = serde_json::from_str::<PrincipalId>("\"abc.def\"").expect_err("dot is unsafe");
    assert!(err.to_string().contains("invalid character"), "{err}");
}
