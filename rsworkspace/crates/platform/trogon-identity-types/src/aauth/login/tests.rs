use super::*;

#[test]
fn login_request_query_string_round_trip() {
    let req = LoginRequest {
        ps: "https://ps.example".into(),
        tenant: Some("corp".into()),
        login_hint: Some("user@corp.example".into()),
        domain_hint: None,
        start_path: Some("/projects/tokyo-trip".into()),
    };
    let query = req.to_query_string();
    let parsed = LoginRequest::parse_query_string(&query).unwrap();
    assert_eq!(parsed, req);
}

#[test]
fn login_request_matches_draft_example_url() {
    let query =
        "ps=https%3A%2F%2Fps.example&tenant=corp&login_hint=user%40corp.example&start_path=%2Fprojects%2Ftokyo-trip";
    let parsed = LoginRequest::parse_query_string(query).unwrap();
    assert_eq!(parsed.ps, "https://ps.example");
    assert_eq!(parsed.tenant.as_deref(), Some("corp"));
    assert_eq!(parsed.login_hint.as_deref(), Some("user@corp.example"));
    assert_eq!(parsed.start_path.as_deref(), Some("/projects/tokyo-trip"));
}

#[test]
fn login_request_serde_round_trip() {
    let req = LoginRequest::new("https://ps.example");
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json, serde_json::json!({"ps": "https://ps.example"}));
    let back: LoginRequest = serde_json::from_value(json).unwrap();
    assert_eq!(back, req);
}

#[test]
fn login_request_minimal_query_string_only_ps() {
    let req = LoginRequest::new("https://ps.example");
    assert_eq!(req.to_query_string(), "ps=https%3A%2F%2Fps.example");
}

#[test]
fn parse_query_string_survives_percent_before_multibyte_character() {
    // A percent sign directly followed by a multi-byte code point used
    // to panic via a mid-character &str slice in urldecode.
    let parsed = LoginRequest::parse_query_string("ps=%\u{20ac}&login_hint=a%zzb");
    let parsed = parsed.expect("malformed escapes fall through as literals");
    assert_eq!(parsed.ps, "%\u{20ac}");
    assert_eq!(parsed.login_hint.as_deref(), Some("a%zzb"));
}

#[test]
fn to_query_string_includes_domain_hint_when_present() {
    let req = LoginRequest {
        ps: "https://ps.example".into(),
        login_hint: None,
        domain_hint: Some("corp.example".into()),
        tenant: None,
        start_path: None,
    };
    let query = req.to_query_string();
    assert_eq!(query, "ps=https%3A%2F%2Fps.example&domain_hint=corp.example");
    let parsed = LoginRequest::parse_query_string(&query).unwrap();
    assert_eq!(parsed, req);
}

#[test]
fn parse_query_string_skips_empty_pairs_from_stray_ampersands() {
    let parsed = LoginRequest::parse_query_string("ps=https%3A%2F%2Fps.example&&").unwrap();
    assert_eq!(parsed.ps, "https://ps.example");
}

#[test]
fn parse_query_string_ignores_unknown_parameters() {
    let parsed = LoginRequest::parse_query_string("ps=https%3A%2F%2Fps.example&unknown=1").unwrap();
    assert_eq!(parsed.ps, "https://ps.example");
}
