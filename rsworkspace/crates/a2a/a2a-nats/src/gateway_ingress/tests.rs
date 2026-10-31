use async_nats::header::HeaderMap;
use jsonrpc_nats::{Direction, decode, to_json_value};
use trogon_identity_types::{PrincipalId, PrincipalKind, SubjectPrincipal};

use super::*;
use crate::a2a_method::A2aMethod;
use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::nats::subjects::agents::AgentDeliverySubject;
use crate::nats::subjects::gateway::GatewayIngressSubject;

fn pfx() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

#[test]
fn ingress_from_agent_subject_keeps_the_principal_position() {
    assert_eq!(
        gateway_ingress_subject_from_agent_subject("a2a.v1.agents.planner.person.alice.message.stream", &pfx())
            .unwrap(),
        "a2a.v1.gateway.planner.person.alice.message.stream"
    );
}

#[test]
fn ingress_from_agent_wrong_leader_returns_none() {
    assert!(gateway_ingress_subject_from_agent_subject("a2a.v1.gateway.x.person.alice.message.send", &pfx()).is_none());
    assert!(gateway_ingress_subject_from_agent_subject("wrong.agent.x.message.send", &pfx()).is_none());
    assert!(gateway_ingress_subject_from_agent_subject("a2a.v1.agents.", &pfx()).is_none());
}

#[test]
fn the_gateway_relays_an_overlaid_subject_back_to_the_agent_subject_it_came_from() {
    let agent = A2aAgentId::new("planner").unwrap();
    let callers = [
        SubjectPrincipal::from_tokens("person", "alice").unwrap(),
        SubjectPrincipal::from_principal(PrincipalKind::Service, &PrincipalId::new("svc.billing").unwrap()),
    ];
    for prefix in [pfx(), A2aPrefix::new("my.app").unwrap()] {
        for caller in &callers {
            for method in A2aMethod::ALL {
                let delivery = AgentDeliverySubject::new(&prefix, &agent, caller, method);
                let ingress = gateway_ingress_subject_from_agent_subject(&delivery.to_string(), &prefix).unwrap();
                let parsed = GatewayIngressSubject::parse(&prefix, &ingress).unwrap();
                assert_eq!(parsed.caller(), caller);
                assert_eq!(parsed.agent_delivery(), delivery, "{ingress}");
            }
        }
    }
}

#[test]
fn invalid_request_payload_produces_stable_jsonrpc_wrapper() {
    let headers = HeaderMap::new();
    let hint = br#"{"jsonrpc":"2.0","id":"x","method":"m"}"#;
    let encoded = ingress_error_response_wire(&headers, hint, -32600, "bad ingress", None);
    let value = to_json_value(&decode(Direction::Response, None, &encoded.headers, &encoded.body).unwrap());
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], "x");
    assert_eq!(value["error"]["code"], -32600);
    assert!(value["error"]["message"].as_str().unwrap().contains("bad ingress"));
}

fn parse_error_code(encoded: &jsonrpc_nats::Encoded) -> i64 {
    let value = to_json_value(&decode(Direction::Response, None, &encoded.headers, &encoded.body).unwrap());
    value["error"]["code"].as_i64().unwrap()
}

#[test]
fn policy_denied_emits_code_minus_32801() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(&headers, b"{}", -32_801, "denied", None);
    assert_eq!(parse_error_code(&encoded), -32_801);
}

#[test]
fn declarative_denied_emits_code_minus_32803() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(&headers, b"{}", -32_803, "tier1", None);
    assert_eq!(parse_error_code(&encoded), -32_803);
}

#[test]
fn error_wire_carries_the_code_header_the_binding_discriminates_on() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(&headers, b"{}", -32_801, "denied", None);
    // The JSON-RPC-over-NATS binding discriminates errors via this header;
    // a deny reply that drops it is unparseable as an error.
    let code_header = encoded
        .headers
        .get(jsonrpc_nats::HEADER_ERROR_CODE)
        .expect("error code header present");
    assert_eq!(code_header.as_str(), "-32801");
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
    );
    let value = to_json_value(&decode(Direction::Response, None, &encoded.headers, &encoded.body).unwrap());
    assert_eq!(value["error"]["code"], -32_802);
    assert_eq!(value["error"]["data"]["rule"], "no-pii");
    assert_eq!(
        ingress_gateway_tier3_refused_response_bytes(&headers, b"{}", "refused", "no-pii"),
        encoded.body
    );
}

#[test]
fn deadline_exceeded_emits_code_minus_32800() {
    let headers = HeaderMap::new();
    let encoded = ingress_error_response_wire(&headers, b"{}", -32_800, "timeout", None);
    assert_eq!(parse_error_code(&encoded), -32_800);
}
