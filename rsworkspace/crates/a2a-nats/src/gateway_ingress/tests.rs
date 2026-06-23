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
    let hint = br#"{"jsonrpc":"2.0","id":"x","method":"m"}"#;
    let bytes = ingress_invalid_request_response_bytes(hint, "bad ingress").unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
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
    assert!(
        GatewayIngressError::NotGatewayIngress
            .to_string()
            .contains("does not start")
    );
    assert!(GatewayIngressError::BadSubjectShape.to_string().contains("expected"));
    assert!(
        GatewayIngressError::UnknownMethodSuffix
            .to_string()
            .contains("unknown method")
    );
    assert!(GatewayIngressError::InvalidAgentId.to_string().contains("agent id"));
    assert!(
        GatewayComposeError::EmptyMethodTail
            .to_string()
            .contains("method suffix is empty")
    );
    assert!(
        GatewayComposeError::UnknownMethodSuffix
            .to_string()
            .contains("not a recognised")
    );
}

fn parse_error_code(bytes: &[u8]) -> i64 {
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["error"]["code"].as_i64().unwrap()
}

#[test]
fn policy_denied_emits_code_minus_32801() {
    let bytes = ingress_gateway_policy_denied_response_bytes(b"{}", "denied").unwrap();
    assert_eq!(parse_error_code(&bytes), -32_801);
}

#[test]
fn declarative_denied_emits_code_minus_32803() {
    let bytes = ingress_gateway_declarative_denied_response_bytes(b"{}", "tier1").unwrap();
    assert_eq!(parse_error_code(&bytes), -32_803);
}

#[test]
fn tier3_refused_emits_code_minus_32802_with_rule() {
    let bytes = ingress_gateway_tier3_refused_response_bytes(b"{}", "refused", "no-pii").unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], -32_802);
    assert_eq!(value["error"]["data"]["rule"], "no-pii");
}

#[test]
fn deadline_exceeded_emits_code_minus_32800() {
    let bytes = ingress_gateway_deadline_exceeded_response_bytes(b"{}", "timeout").unwrap();
    assert_eq!(parse_error_code(&bytes), -32_800);
}
