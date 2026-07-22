use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;

fn fake_jwt_with_payload(payload_json: &serde_json::Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"ES256\"}");
    let payload = URL_SAFE_NO_PAD.encode(payload_json.to_string());
    format!("{header}.{payload}.sig")
}

#[test]
fn parent_agent_of_returns_none_when_absent() {
    let jwt = fake_jwt_with_payload(&serde_json::json!({"sub": "aauth:assistant@agent.example"}));
    assert_eq!(parent_agent_of(&jwt).unwrap(), None);
}

#[test]
fn parent_agent_of_returns_value_when_present() {
    let jwt = fake_jwt_with_payload(&serde_json::json!({
        "sub": "aauth:sub@agent.example",
        "parent_agent": "aauth:assistant@agent.example"
    }));
    assert_eq!(
        parent_agent_of(&jwt).unwrap(),
        Some("aauth:assistant@agent.example".to_string())
    );
}

#[test]
fn parent_agent_of_rejects_malformed_shape() {
    let err = parent_agent_of("not-a-jwt").unwrap_err();
    assert!(matches!(err, ParentAgentError::BadShape));
}

#[test]
fn parent_agent_of_rejects_bad_base64() {
    let err = parent_agent_of("header.not!base64url.sig").unwrap_err();
    assert!(matches!(err, ParentAgentError::Base64(_)));
}
