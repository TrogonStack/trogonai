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

    claim.claim(&from("discord")).expect("first initialize");
    claim.admit(&from("discord")).expect("same client again");
    claim.claim(&from("discord")).expect("same client again");
}

#[test]
fn a_different_client_is_refused_with_an_error_naming_both() {
    let claim = ClientClaim::default();
    claim.claim(&from("discord")).expect("first initialize");

    let error = claim.admit(&from("zed")).expect_err("second client");

    assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    assert!(error.message.contains("`discord`"), "{}", error.message);
    assert!(error.message.contains("`zed`"), "{}", error.message);
    assert!(error.message.contains("own ACP prefix"), "{}", error.message);
    assert_eq!(
        error.data,
        Some(serde_json::json!({"servedClient": "discord", "refusedClient": "zed"}))
    );
    claim.claim(&from("zed")).expect_err("nor may it claim");
}

#[test]
fn a_refusal_leaves_the_first_client_admitted() {
    let claim = ClientClaim::default();
    claim.claim(&from("discord")).expect("first initialize");
    claim.claim(&from("zed")).expect_err("second client");

    claim.admit(&from("discord")).expect("first client still served");
}

#[test]
fn admitting_an_initialize_claims_nothing() {
    // An initialize the agent then refuses must not lock anyone out.
    let claim = ClientClaim::default();
    claim.admit(&from("squatter")).expect("nothing claimed yet");

    claim.claim(&from("discord")).expect("the real client claims");
}

#[test]
fn a_client_without_client_info_is_its_own_identity() {
    let claim = ClientClaim::default();
    claim.claim(&anonymous()).expect("first initialize");
    claim.admit(&anonymous()).expect("anonymous again");

    let error = claim.admit(&from("zed")).expect_err("named client");
    assert!(error.message.contains("(no clientInfo)"), "{}", error.message);
}

fn header(claim: &ClientClaim, name: Option<&str>) -> Result<(), Error> {
    ClientGate::admit(claim, name)
}

#[test]
fn a_request_naming_another_client_is_refused() {
    let claim = ClientClaim::default();
    claim.claim(&from("discord")).expect("first initialize");

    header(&claim, Some("discord")).expect("the claiming client");
    let error = header(&claim, Some("zed")).expect_err("another client");

    assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    assert!(error.message.contains("`zed`"), "{}", error.message);
}

#[test]
fn a_request_without_a_name_is_served_and_claims_nothing() {
    let claim = ClientClaim::default();

    header(&claim, None).expect("unlabelled before any claim");
    claim.claim(&from("discord")).expect("the first to initialize claims");
    header(&claim, None).expect("unlabelled after the claim");
}

#[test]
fn a_named_request_is_served_before_any_claim_but_claims_nothing() {
    // One stray request naming a client, sent before the real client
    // initializes, must not lock the real client out. It is still served, as
    // the requests of a client that carried on across a host restart are.
    let claim = ClientClaim::default();
    header(&claim, Some("squatter")).expect("served");

    claim
        .admit(&from("discord"))
        .expect("the real client is not locked out");
    claim.claim(&from("discord")).expect("and claims");
    header(&claim, Some("squatter")).expect_err("the stray client is refused from then on");
}

#[test]
fn an_anonymous_claim_refuses_named_requests() {
    let claim = ClientClaim::default();
    claim.claim(&anonymous()).expect("first initialize");

    header(&claim, Some("zed")).expect_err("a named client");
}
