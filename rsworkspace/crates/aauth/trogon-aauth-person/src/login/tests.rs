use super::*;

#[test]
fn parse_login_request_extracts_required_ps() {
    let query = "ps=https%3A%2F%2Fps.example&tenant=corp";
    let req = parse_login_request(query).unwrap();
    assert_eq!(req.ps, "https://ps.example");
    assert_eq!(req.tenant.as_deref(), Some("corp"));
}

#[test]
fn parse_login_request_returns_none_without_ps() {
    let query = "tenant=corp";
    assert!(parse_login_request(query).is_none());
}

#[test]
fn login_outcome_carries_resource_token_and_optional_redirect() {
    let outcome = LoginOutcome {
        resource_token: "eyJ...".to_string(),
        redirect_back_to: Some("/projects/tokyo-trip".to_string()),
    };
    assert_eq!(outcome.resource_token, "eyJ...");
    assert_eq!(outcome.redirect_back_to.as_deref(), Some("/projects/tokyo-trip"));
}
