use jsonwebtoken::{Algorithm, Header, encode};

use super::*;
use crate::test_support::key_fixture;

#[test]
fn parent_agent_of_returns_none_when_absent() {
    let key = key_fixture("ap-kid-1");
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("ap-kid-1".into());
    let claims = serde_json::json!({
        "iss": "https://ap.example",
        "sub": "aauth:asst@agent.example",
        "jti": "agent-jti-1",
        "iat": 0,
        "exp": 1000,
    });
    let jwt = encode(&header, &claims, &key.encoding_key).unwrap();

    assert_eq!(parent_agent_of(&jwt).unwrap(), None);
}

#[test]
fn parent_agent_of_returns_value_when_present() {
    let key = key_fixture("ap-kid-2");
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("ap-kid-2".into());
    let claims = serde_json::json!({
        "iss": "https://ap.example",
        "sub": "aauth:sub@agent.example",
        "jti": "agent-jti-2",
        "iat": 0,
        "exp": 1000,
        "parent_agent": "aauth:parent@agent.example",
    });
    let jwt = encode(&header, &claims, &key.encoding_key).unwrap();

    assert_eq!(
        parent_agent_of(&jwt).unwrap(),
        Some("aauth:parent@agent.example".to_string())
    );
}

#[test]
fn parent_agent_of_rejects_malformed_jwt_shape() {
    let err = parent_agent_of("not-a-jwt").unwrap_err();
    assert!(matches!(err, ParentAgentError::BadShape));
}

#[test]
fn parent_agent_of_rejects_bad_base64_payload() {
    let err = parent_agent_of("header.not$$base64url.sig").unwrap_err();
    assert!(matches!(err, ParentAgentError::Base64(_)));
}
