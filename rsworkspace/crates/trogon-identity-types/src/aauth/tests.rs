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
