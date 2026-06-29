use super::*;

#[test]
fn caller_https_auth_display_redacts() {
    let auth = CallerHttpsAuth::new("Bearer secret-token");
    assert_eq!(format!("{auth}"), "<redacted>");
    assert_eq!(auth.as_str(), "Bearer secret-token");
}

#[test]
fn bridge_user_jwt_rejects_empty_and_non_three_segment() {
    assert!(BridgeUserJwt::new("").is_err());
    assert!(BridgeUserJwt::new("a.b").is_err());
    assert!(BridgeUserJwt::new("a..c").is_err());
    assert!(BridgeUserJwt::new("a.b.c").is_ok());
}

#[test]
fn bridge_user_jwt_display_redacts() {
    let jwt = BridgeUserJwt::new("h.p.s").expect("valid shape");
    assert_eq!(format!("{jwt}"), "<redacted>");
}

#[test]
fn debug_does_not_leak_secrets() {
    let auth = CallerHttpsAuth::new("Bearer secret-token");
    let auth_dbg = format!("{auth:?}");
    assert!(!auth_dbg.contains("secret-token"), "{auth_dbg}");
    assert!(auth_dbg.contains("<redacted>"), "{auth_dbg}");

    let jwt = BridgeUserJwt::new("hhh.ppp.sss").expect("valid shape");
    let jwt_dbg = format!("{jwt:?}");
    assert!(!jwt_dbg.contains("hhh"), "{jwt_dbg}");
    assert!(jwt_dbg.contains("<redacted>"), "{jwt_dbg}");
}

#[test]
fn minted_caller_id_round_trips_from_auth_callout_caller_id() {
    let caller = a2a_auth_callout::CallerId::new("bridge-caller").unwrap();
    let minted = MintedCallerId::from_caller_id(caller);
    assert_eq!(minted.as_str(), "bridge-caller");
}

#[test]
fn bridge_agent_id_parse_and_display() {
    let agent = BridgeAgentId::parse("my-agent").unwrap();
    assert_eq!(agent.as_str(), "my-agent");
    assert_eq!(agent.to_string(), "my-agent");
    assert_eq!(agent.as_agent_id().as_str(), "my-agent");
    assert_eq!(agent.clone().into_agent_id().as_str(), "my-agent");

    let err = BridgeAgentId::parse("bad agent").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidAgent(_)));
}

#[test]
fn bridge_user_jwt_trims_and_into_inner() {
    let jwt = BridgeUserJwt::new("  a.b.c  ").expect("valid jwt");
    assert_eq!(jwt.as_str(), "a.b.c");
    assert_eq!(jwt.into_inner(), "a.b.c");
}
