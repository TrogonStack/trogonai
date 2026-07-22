use async_nats::header::HeaderMap;
use jsonrpc_nats::{Direction, decode, to_json_value};

use super::*;
use crate::a2a_prefix::A2aPrefix;

fn pfx() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

#[test]
fn message_send() {
    assert_eq!(
        resolve_gateway_ingress_subject("a2a.gateway.bot.message.send", &pfx()).unwrap(),
        "a2a.agents.bot.message.send"
    );
}

#[test]
fn legacy_two_segment_identity_area_rejected() {
    assert!(matches!(
        resolve_gateway_ingress_subject("a2a.gateway.acme.bot.message.send", &pfx()),
        Err(GatewayIngressError::UnknownMethodSuffix)
    ));
}

#[test]
fn push_set_two_token_suffix() {
    assert_eq!(
        resolve_gateway_ingress_subject("a2a.gateway.planner.push.set", &pfx()).unwrap(),
        "a2a.agents.planner.push.set"
    );
}

#[test]
fn dotted_prefix() {
    let p = A2aPrefix::new("my.app").unwrap();
    assert_eq!(
        resolve_gateway_ingress_subject("my.app.gateway.planner.tasks.get", &p).unwrap(),
        "my.app.agents.planner.tasks.get"
    );
}

#[test]
fn wrong_prefix_returns_not_gateway() {
    assert!(matches!(
        resolve_gateway_ingress_subject("other.gateway.bot.message.send", &pfx()),
        Err(GatewayIngressError::NotGatewayIngress)
    ));
}

#[test]
fn invalid_agent_id_rejected_instead_of_silent_typo_subject() {
    assert!(matches!(
        resolve_gateway_ingress_subject("a2a.gateway.bad*agent.message.send", &pfx()),
        Err(GatewayIngressError::InvalidAgentId)
    ));
}

#[test]
fn too_many_segments_before_suffix_rejected() {
    assert!(matches!(
        resolve_gateway_ingress_subject("a2a.gateway.t1.t2.bot.message.send", &pfx()),
        Err(GatewayIngressError::UnknownMethodSuffix)
    ));
}

#[test]
fn compose_then_resolve_round_trips() {
    let p = pfx();
    let aid = A2aAgentId::new("planner").unwrap();
    let g = compose_gateway_ingress_subject(&p, &aid, "message.send").unwrap();
    assert_eq!(g, "a2a.gateway.planner.message.send");
    assert_eq!(
        resolve_gateway_ingress_subject(&g, &p).unwrap(),
        "a2a.agents.planner.message.send"
    );
}

#[test]
fn ingress_from_agent_subject_transform() {
    let p = pfx();
    assert_eq!(
        gateway_ingress_subject_from_agent_subject("a2a.agents.planner.message.stream", &p).unwrap(),
        "a2a.gateway.planner.message.stream"
    );
}

#[test]
fn ingress_from_agent_wrong_leader_returns_none() {
    assert!(gateway_ingress_subject_from_agent_subject("a2a.gateway.x.message.send", &pfx()).is_none());
    assert!(gateway_ingress_subject_from_agent_subject("wrong.agent.x.message.send", &pfx()).is_none());
}

#[test]
fn ingress_agent_method_matches_resolve_subject() {
    let p = pfx();
    let subject = "a2a.gateway.planner.message.send";
    let (agent, method_dots) = gateway_ingress_agent_and_method_dots(subject, &p).unwrap();
    assert_eq!(agent.as_str(), "planner");
    assert_eq!(method_dots, "message.send");
    assert_eq!(
        resolve_gateway_ingress_subject(subject, &p).unwrap(),
        "a2a.agents.planner.message.send"
    );
}

#[test]
fn compose_rejects_blank_method_suffix() {
    let p = pfx();
    let aid = A2aAgentId::new("b").unwrap();
    assert!(matches!(
        compose_gateway_ingress_subject(&p, &aid, ""),
        Err(GatewayComposeError::EmptyMethodTail)
    ));
    assert!(matches!(
        compose_gateway_ingress_subject(&p, &aid, "..."),
        Err(GatewayComposeError::EmptyMethodTail)
    ));
}

#[test]
fn invalid_request_payload_produces_stable_jsonrpc_wrapper() {
    let headers = HeaderMap::new();
    let hint = br#"{"jsonrpc":"2.0","id":"x","method":"m"}"#;
    let encoded = ingress_error_response_wire(&headers, hint, -32600, "bad ingress", None).unwrap();
    let value = to_json_value(&decode(Direction::Response, None, &encoded.headers, &encoded.body).unwrap());
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], "x");
    assert_eq!(value["error"]["code"], -32600);
    assert!(value["error"]["message"].as_str().unwrap().contains("bad ingress"));
}

#[test]
fn empty_rest_after_gateway_leader_is_bad_shape() {
    assert!(matches!(
        resolve_gateway_ingress_subject("a2a.gateway.", &pfx()),
        Err(GatewayIngressError::BadSubjectShape)
    ));
    assert!(matches!(
        gateway_ingress_agent_and_method_dots("a2a.gateway.", &pfx()),
        Err(GatewayIngressError::BadSubjectShape)
    ));
}

#[test]
fn no_known_suffix_matches_returns_unknown_method() {
    assert!(matches!(
        resolve_gateway_ingress_subject("a2a.gateway.bot.foo.bar", &pfx()),
        Err(GatewayIngressError::UnknownMethodSuffix)
    ));
}

#[test]
fn compose_rejects_unknown_method_suffix() {
    let p = pfx();
    let aid = A2aAgentId::new("b").unwrap();
    assert!(matches!(
        compose_gateway_ingress_subject(&p, &aid, "message.sned"),
        Err(GatewayComposeError::UnknownMethodSuffix)
    ));
    assert!(matches!(
        compose_gateway_ingress_subject(&p, &aid, "tasks"),
        Err(GatewayComposeError::UnknownMethodSuffix)
    ));
}

#[test]
fn ingress_error_display_covers_every_variant() {
    assert_eq!(
        GatewayIngressError::NotGatewayIngress.to_string(),
        "subject does not start with '{prefix}.gateway.' for the configured prefix"
    );
    assert_eq!(
        GatewayIngressError::BadSubjectShape.to_string(),
        "expected '{prefix}.gateway.{agent_id}.{method…}'"
    );
    assert_eq!(
        GatewayIngressError::UnknownMethodSuffix.to_string(),
        "unknown method suffix after gateway segment"
    );
    assert_eq!(
        GatewayIngressError::InvalidAgentId.to_string(),
        "agent id segment fails NATS token validation"
    );
    assert_eq!(
        GatewayComposeError::EmptyMethodTail.to_string(),
        "gateway ingress method suffix is empty"
    );
    assert_eq!(
        GatewayComposeError::UnknownMethodSuffix.to_string(),
        "gateway ingress method suffix is not a recognised A2A operation"
    );
}

fn parse_error_code(encoded: &jsonrpc_nats::Encoded) -> i64 {
    let value = to_json_value(&decode(Direction::Response, None, &encoded.headers, &encoded.body).unwrap());
    value["error"]["code"].as_i64().unwrap()
}

#[test]
fn policy_denied_emits_code_minus_32801() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(&headers, b"{}", -32_801, "denied", None).unwrap();
    assert_eq!(parse_error_code(&encoded), -32_801);
}

#[test]
fn declarative_denied_emits_code_minus_32803() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(&headers, b"{}", -32_803, "tier1", None).unwrap();
    assert_eq!(parse_error_code(&encoded), -32_803);
}

#[test]
fn aauth_denied_emits_code_minus_32118() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(&headers, b"{}", -32_118, "aauth", None).unwrap();
    assert_eq!(parse_error_code(&encoded), -32_118);
    // The JSON-RPC-over-NATS binding discriminates errors via this header;
    // a deny reply that drops it is unparseable as an error.
    let code_header = encoded
        .headers
        .get(jsonrpc_nats::HEADER_ERROR_CODE)
        .expect("error code header present");
    assert_eq!(code_header.as_str(), "-32118");
}

#[test]
fn aauth_denied_response_bytes_emits_code_minus_32118() {
    // `ingress_gateway_aauth_denied_response_bytes` returns only the body
    // (the -32118 code lives in the NATS header `ingress_error_wire` sets,
    // which this body-only helper intentionally discards for callers that
    // reply through a channel carrying its own headers), so this asserts the
    // message round-trips and cross-checks the code against the
    // header-carrying twin built from the same underlying encoder.
    let headers = HeaderMap::new();
    let bytes = ingress_gateway_aauth_denied_response_bytes(&headers, b"{}", "aauth required").unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["message"], "aauth required");

    let wire = ingress_error_response_wire(&headers, b"{}", -32_118, "aauth required", None).unwrap();
    assert_eq!(wire.body.as_ref(), bytes.as_ref());
    assert_eq!(
        wire.headers
            .get(jsonrpc_nats::HEADER_ERROR_CODE)
            .map(|v| v.as_str().to_owned()),
        Some("-32118".to_owned())
    );
}

#[test]
fn tier3_refused_emits_code_minus_32802_with_rule() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(
        &headers,
        b"{}",
        -32_802,
        "refused",
        Some(serde_json::json!({ "rule": "no-pii" })),
    )
    .unwrap();
    let value = to_json_value(&decode(Direction::Response, None, &encoded.headers, &encoded.body).unwrap());
    assert_eq!(value["error"]["code"], -32_802);
    assert_eq!(value["error"]["data"]["rule"], "no-pii");
}

#[test]
fn deadline_exceeded_emits_code_minus_32800() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(&headers, b"{}", -32_800, "timeout", None).unwrap();
    assert_eq!(parse_error_code(&encoded), -32_800);
}
