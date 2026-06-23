use super::*;

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

fn agent_id() -> A2aAgentId {
    A2aAgentId::new("test-agent").unwrap()
}

fn minted_jwt() -> MintedUserJwt {
    MintedUserJwt::new("aaa.bbb.ccc").unwrap()
}

#[test]
fn new_uses_agent_subjects_by_default() {
    let client = A2aClient::new(prefix(), agent_id(), (), ());
    assert!(matches!(client.ingress, ClientIngressTarget::AgentSubjects));
    assert!(client.gateway_caller_jwt().is_none());
}

#[test]
fn new_uses_default_operation_timeout() {
    let client = A2aClient::new(prefix(), agent_id(), (), ());
    assert_eq!(client.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
}

#[test]
fn with_operation_timeout_overrides_default() {
    let client = A2aClient::new(prefix(), agent_id(), (), ()).with_operation_timeout(Duration::from_secs(7));
    assert_eq!(client.operation_timeout(), Duration::from_secs(7));
}

#[test]
fn with_operation_timeout_clamps_below_minimum_to_minimum() {
    let client = A2aClient::new(prefix(), agent_id(), (), ()).with_operation_timeout(Duration::ZERO);
    assert_eq!(client.operation_timeout(), Duration::from_secs(MIN_TIMEOUT_SECS));
}

#[test]
fn routing_via_gateway_ingress_stores_jwt() {
    let client = A2aClient::new(prefix(), agent_id(), (), ()).routing_via_gateway_ingress(minted_jwt());
    assert!(matches!(client.ingress, ClientIngressTarget::GatewayIngress(_)));
    assert!(client.gateway_caller_jwt().is_some());
}

#[test]
fn routing_to_agent_flips_back_to_agent_subjects() {
    let client = A2aClient::new(prefix(), agent_id(), (), ())
        .routing_via_gateway_ingress(minted_jwt())
        .routing_to_agent();
    assert!(matches!(client.ingress, ClientIngressTarget::AgentSubjects));
}

#[test]
fn agent_id_accessor_returns_constructor_value() {
    let client = A2aClient::new(prefix(), agent_id(), (), ());
    assert_eq!(client.agent_id().as_str(), "test-agent");
}

#[test]
fn outbound_rpc_subject_returns_agent_subject_for_default_routing() {
    let client = A2aClient::new(prefix(), agent_id(), (), ());
    let subject = client
        .outbound_rpc_subject("a2a.agents.test-agent.message.send".to_string())
        .unwrap();
    assert_eq!(subject, "a2a.agents.test-agent.message.send");
}

#[test]
fn outbound_rpc_subject_swaps_agents_to_gateway_when_jwt_set() {
    let client = A2aClient::new(prefix(), agent_id(), (), ()).routing_via_gateway_ingress(minted_jwt());
    let subject = client
        .outbound_rpc_subject("a2a.agents.test-agent.message.send".to_string())
        .unwrap();
    assert_eq!(subject, "a2a.gateway.test-agent.message.send");
}

#[test]
fn outbound_rpc_subject_returns_invalid_overlay_for_non_agent_subject() {
    let client = A2aClient::new(prefix(), agent_id(), (), ()).routing_via_gateway_ingress(minted_jwt());
    let err = client
        .outbound_rpc_subject("wrong.prefix.test-agent.message.send".to_string())
        .unwrap_err();
    assert!(matches!(err, ClientError::InvalidRpcSubjectOverlay));
}

#[test]
fn prefix_accessor_returns_constructor_value() {
    let client = A2aClient::new(prefix(), agent_id(), (), ());
    assert_eq!(client.prefix().as_str(), "a2a");
}

mod agent_card_op;
mod message_send_op;
mod message_stream_op;
mod push_delete_op;
mod push_get_op;
mod push_list_op;
mod push_set_op;
mod tasks_cancel_op;
mod tasks_get_op;
mod tasks_list_op;
mod tasks_resubscribe_op;
