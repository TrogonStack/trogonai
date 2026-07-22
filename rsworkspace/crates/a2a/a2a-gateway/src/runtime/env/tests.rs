use trogon_std::env::InMemoryEnv;

use super::*;

#[test]
fn tier2_cel_enabled_recognizes_padded_truthy_values() {
    for raw in ["1", "true", "yes", "on", " true ", "\tYES\n"] {
        let env = InMemoryEnv::new();
        env.set(ENV_GATEWAY_TIER2_CEL_ENABLED, raw);
        assert!(gateway_tier2_cel_enabled(&env), "{raw:?} must enable");
    }
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_TIER2_CEL_ENABLED, "off");
    assert!(!gateway_tier2_cel_enabled(&env));
    let empty = InMemoryEnv::new();
    assert!(!gateway_tier2_cel_enabled(&empty));
}

#[test]
fn audit_publish_defaults_to_disabled_when_unset() {
    let env = InMemoryEnv::new();
    assert!(!gateway_audit_publish_enabled(&env));
}

#[test]
fn audit_publish_enables_on_truthy_value() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_AUDIT_PUBLISH, "on");
    assert!(gateway_audit_publish_enabled(&env));
}

#[test]
fn tier3_signing_pubkey_returns_none_when_unset() {
    let env = InMemoryEnv::new();
    assert!(gateway_tier3_signing_pubkey(&env).is_none());
}

#[test]
fn tier3_signing_pubkey_returns_none_for_empty_string() {
    // An empty value must surface as "no pubkey configured" rather
    // than a half-trusted invalid pubkey. Operators clearing the
    // env var to disable signing rely on this.
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_TIER3_SIGNING_PUBKEY, "   ");
    assert!(gateway_tier3_signing_pubkey(&env).is_none());
}

#[test]
fn tier3_signing_pubkey_returns_none_for_invalid_hex() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_TIER3_SIGNING_PUBKEY, "not-hex");
    assert!(gateway_tier3_signing_pubkey(&env).is_none());
}

#[test]
fn tier3_signing_pubkey_parses_valid_hex() {
    // Test ed25519 pubkey from the a2a-redaction fixtures (32-byte
    // hex). Asserts the success path produces a `Some(_)` without
    // hard-coding the inner type's debug shape.
    let env = InMemoryEnv::new();
    let hex = "abababababababababababababababababababababababababababababababab";
    env.set(ENV_GATEWAY_TIER3_SIGNING_PUBKEY, hex);
    assert!(gateway_tier3_signing_pubkey(&env).is_some());
}

#[test]
fn unary_deadline_returns_none_for_non_message_send() {
    let env = InMemoryEnv::new();
    assert!(unary_deadline_for_method(&env, "tasks.get").is_none());
    assert!(unary_deadline_for_method(&env, "message.stream").is_none());
    assert!(unary_deadline_for_method(&env, "card").is_none());
}

#[test]
fn unary_deadline_defaults_to_operation_timeout_when_unset() {
    let env = InMemoryEnv::new();
    let deadline = unary_deadline_for_method(&env, "message.send").expect("message.send carries a deadline");
    assert_eq!(deadline, DEFAULT_OPERATION_TIMEOUT);
}

#[test]
fn unary_deadline_reads_env_override() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_UNARY_DEADLINE_SECS, "120");
    let deadline = unary_deadline_for_method(&env, "message.send").expect("present");
    assert_eq!(deadline, Duration::from_secs(120));
}

#[test]
fn unary_deadline_clamps_zero_to_one_second() {
    // A literal zero would let the dispatch path return immediately
    // before the agent even sees the request -- clamp to 1s so a
    // misconfigured deadline still gives the agent a chance to
    // respond. Operators wanting "no deadline" use a large value.
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_UNARY_DEADLINE_SECS, "0");
    let deadline = unary_deadline_for_method(&env, "message.send").expect("present");
    assert_eq!(deadline, Duration::from_secs(1));
}

#[test]
fn unary_deadline_ignores_garbage_env_value() {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_UNARY_DEADLINE_SECS, "not-a-number");
    let deadline = unary_deadline_for_method(&env, "message.send").expect("present");
    assert_eq!(deadline, DEFAULT_OPERATION_TIMEOUT);
}

#[test]
fn unix_epoch_ms_returns_positive_value() {
    // Wall-clock should always be after the epoch on any sane
    // host. A zero return would indicate `SystemTime` panicked
    // back below epoch -- worth catching.
    assert!(unix_epoch_ms() > 0);
}

#[test]
fn json_rpc_params_returns_empty_object_for_malformed_payload() {
    let params = json_rpc_params(b"not-json");
    assert_eq!(params, serde_json::Value::Object(Default::default()));
}

#[test]
fn json_rpc_params_returns_empty_object_when_field_missing() {
    let payload = br#"{"jsonrpc":"2.0","id":"1","method":"tasks/get"}"#;
    let params = json_rpc_params(payload);
    assert_eq!(params, serde_json::Value::Object(Default::default()));
}

#[test]
fn json_rpc_params_extracts_present_params_object() {
    let payload = br#"{"jsonrpc":"2.0","id":"1","method":"tasks/get","params":{"id":"t-1"}}"#;
    let params = json_rpc_params(payload);
    assert_eq!(params["id"], "t-1");
}

#[test]
fn json_rpc_params_normalizes_array_to_empty_object() {
    // The helper's contract is "always returns an object". A
    // payload with `params: []` (positional JSON-RPC) would
    // otherwise leak an array into Tier-3 manifest-lookup
    // codepaths that .get(key) on a Value::Object.
    let payload = br#"{"jsonrpc":"2.0","id":"1","method":"x","params":[]}"#;
    assert_eq!(json_rpc_params(payload), serde_json::Value::Object(Default::default()));
}

#[test]
fn json_rpc_params_normalizes_null_to_empty_object() {
    let payload = br#"{"jsonrpc":"2.0","id":"1","method":"x","params":null}"#;
    assert_eq!(json_rpc_params(payload), serde_json::Value::Object(Default::default()));
}

#[test]
fn json_rpc_params_normalizes_scalar_to_empty_object() {
    let payload = br#"{"jsonrpc":"2.0","id":"1","method":"x","params":42}"#;
    assert_eq!(json_rpc_params(payload), serde_json::Value::Object(Default::default()));
}

#[test]
fn json_rpc_audit_req_id_returns_none_for_null_id() {
    // A JSON-RPC envelope with `"id": null` mustn't synthesize a
    // correlation key -- the audit consumer joins on this and
    // `"null"` would alias unrelated null-id envelopes onto the
    // same row.
    let payload = br#"{"jsonrpc":"2.0","id":null,"method":"x","params":{}}"#;
    assert!(json_rpc_audit_req_id(payload).is_none());
}

#[test]
fn json_rpc_audit_req_id_returns_none_for_notification() {
    // A JSON-RPC notification (no id field) must not synthesize a
    // correlation key -- the audit consumer joins on this and a
    // fake id would alias unrelated notifications onto the same row.
    let payload = br#"{"jsonrpc":"2.0","method":"tasks/get","params":{}}"#;
    assert!(json_rpc_audit_req_id(payload).is_none());
}

#[test]
fn json_rpc_audit_req_id_returns_request_id_when_present() {
    let payload = br#"{"jsonrpc":"2.0","id":"req-42","method":"tasks/get","params":{}}"#;
    assert_eq!(json_rpc_audit_req_id(payload).as_deref(), Some("req-42"));
}
