use super::*;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::Implementation;

fn from(name: &str) -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new(name, "1.0.0"))
}

fn anonymous() -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1)
}

#[test]
fn the_first_client_claims_the_host_and_may_initialize_again() {
    let claim = ClientClaim::default();

    claim.admit(&from("discord")).expect("first initialize");
    claim.admit(&from("discord")).expect("same client again");
}

#[test]
fn a_different_client_is_refused_with_an_error_naming_both() {
    let claim = ClientClaim::default();
    claim.admit(&from("discord")).expect("first initialize");

    let error = claim.admit(&from("zed")).expect_err("second client");

    assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    assert!(error.message.contains("`discord`"), "{}", error.message);
    assert!(error.message.contains("`zed`"), "{}", error.message);
    assert!(error.message.contains("own ACP prefix"), "{}", error.message);
    assert_eq!(
        error.data,
        Some(serde_json::json!({"servedClient": "discord", "refusedClient": "zed"}))
    );
}

#[test]
fn a_refusal_leaves_the_first_client_admitted() {
    let claim = ClientClaim::default();
    claim.admit(&from("discord")).expect("first initialize");
    claim.admit(&from("zed")).expect_err("second client");

    claim.admit(&from("discord")).expect("first client still served");
}

#[test]
fn a_client_without_client_info_is_its_own_identity() {
    let claim = ClientClaim::default();
    claim.admit(&anonymous()).expect("first initialize");
    claim.admit(&anonymous()).expect("anonymous again");

    let error = claim.admit(&from("zed")).expect_err("named client");
    assert!(error.message.contains("(no clientInfo)"), "{}", error.message);
}
