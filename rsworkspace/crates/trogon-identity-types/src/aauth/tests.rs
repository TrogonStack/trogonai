use super::*;

#[test]
fn parses_auth_token_requirement() {
    let raw = "requirement=auth-token; resource-token=\"eyJ.AAA\"";
    let req = Requirement::parse(raw);
    assert_eq!(
        req,
        Requirement::AuthToken {
            resource_token: "eyJ.AAA".into()
        }
    );
}

#[test]
fn parses_interaction_requirement() {
    let raw = "requirement=interaction; url=\"https://ps.example/i/123\"; code=\"AB12\"";
    let req = Requirement::parse(raw);
    assert_eq!(
        req,
        Requirement::Interaction {
            url: "https://ps.example/i/123".into(),
            code: Some("AB12".into()),
        }
    );
}

#[test]
fn renders_round_trip_auth_token() {
    let req = Requirement::AuthToken {
        resource_token: "eyJTOK".into(),
    };
    let v = req.to_header_value();
    let again = Requirement::parse(&v);
    assert_eq!(req, again);
}

#[test]
fn agent_claims_serde() {
    let c = AgentClaims {
        iss: "https://ap.example".into(),
        sub: "aauth:agent-1@example".into(),
        jti: "abc".into(),
        iat: 100,
        exp: 200,
        dwk: DWK_AGENT.into(),
        cnf: Cnf {
            jwk: serde_json::json!({"kty": "EC", "crv": "P-256", "x": "X", "y": "Y"}),
        },
        ps: Some("https://ps.example".into()),
    };
    let j = serde_json::to_value(&c).unwrap();
    assert_eq!(j["dwk"], DWK_AGENT);
    assert_eq!(j["cnf"]["jwk"]["kty"], "EC");
}

#[test]
fn nats_envelope_canonical_base_is_stable() {
    let env = NatsSignatureEnvelope {
        token: "TOK".into(),
        sig_input: "(\"@subject\")".into(),
        sig: "SIG".into(),
        created: 1000,
        nonce: "N".into(),
        content_digest: "sha-256=:abc:".into(),
    };
    let a = env.canonical_base("foo.bar", Some("_INBOX.1"), "JKT");
    let b = env.canonical_base("foo.bar", Some("_INBOX.1"), "JKT");
    assert_eq!(a, b);
    assert!(a.contains("\"@subject\": foo.bar"));
    assert!(a.contains("keyid=\"JKT\""));
}

#[test]
fn parses_clarification_and_approval_pending() {
    assert_eq!(
        Requirement::parse("requirement=clarification"),
        Requirement::Clarification
    );
    assert_eq!(
        Requirement::parse("requirement=approval-pending"),
        Requirement::ApprovalPending
    );
}

#[test]
fn parses_interaction_without_code_and_unknown_requirements() {
    assert_eq!(
        Requirement::parse("requirement=interaction; url=\"https://ps.example/i/123\""),
        Requirement::Interaction {
            url: "https://ps.example/i/123".into(),
            code: None,
        }
    );
    let raw = "requirement=custom; foo=bar";
    assert_eq!(Requirement::parse(raw), Requirement::Other { raw: raw.to_string() });
}

#[test]
fn canonical_base_uses_empty_reply_when_missing() {
    let env = NatsSignatureEnvelope {
        token: "TOK".into(),
        sig_input: "(\"@subject\")".into(),
        sig: "SIG".into(),
        created: 1000,
        nonce: "N".into(),
        content_digest: "sha-256=:abc:".into(),
    };
    let base = env.canonical_base("foo.bar", None, "JKT");
    assert!(base.contains("\"@reply\": \n"));
}

#[test]
fn resource_and_auth_claims_serialize_expected_fields() {
    let resource = ResourceClaims {
        iss: "https://resource.example".into(),
        aud: "agent-1".into(),
        jti: "jti".into(),
        iat: 1,
        exp: 2,
        dwk: DWK_RESOURCE.into(),
        agent: "agent-1".into(),
        agent_jkt: "JKT".into(),
        scope: "read".into(),
        mission: Some(MissionRef {
            approver: "alice".into(),
            s256: "hash".into(),
        }),
    };
    let auth = AuthClaims {
        iss: "https://person.example".into(),
        sub: "alice".into(),
        aud: "resource".into(),
        jti: "jti2".into(),
        iat: 1,
        exp: 2,
        agent: "agent-1".into(),
        agent_jkt: "JKT".into(),
        scope: "read".into(),
        principal: Some("alice".into()),
        consent_id: None,
        resource: Some("resource".into()),
        act: None,
        cnf: None,
    };
    assert_eq!(serde_json::to_value(&resource).unwrap()["dwk"], DWK_RESOURCE);
    assert_eq!(serde_json::to_value(&auth).unwrap()["agent"], "agent-1");
}

#[test]
fn requirement_header_round_trips_clarification_and_approval_pending() {
    for req in [Requirement::Clarification, Requirement::ApprovalPending] {
        let header = req.to_header_value();
        assert_eq!(Requirement::parse(&header), req);
    }
}

#[test]
fn aauth_parse_error_display_messages() {
    assert_eq!(
        AAuthParseError::MissingField("token").to_string(),
        "aauth: missing field token"
    );
    assert_eq!(
        AAuthParseError::InvalidNumber("created").to_string(),
        "aauth: invalid number for created"
    );
}

#[test]
fn requirement_parse_unquoted_resource_token() {
    let raw = "requirement=auth-token; resource-token=unquoted-tok";
    let req = Requirement::parse(raw);
    assert_eq!(
        req,
        Requirement::AuthToken {
            resource_token: "unquoted-tok".into()
        }
    );
}

#[test]
fn requirement_other_variant_round_trips() {
    let req = Requirement::Other {
        raw: "requirement=custom".into(),
    };
    assert_eq!(req.to_header_value(), "requirement=custom");
}
