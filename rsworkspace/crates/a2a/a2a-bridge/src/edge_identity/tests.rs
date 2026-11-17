use serde_json::{Value, json};
use trogon_aauth_verify::{VerifiedAgent, VerifiedAuth, VerifiedAuthPresenter};

use super::*;

fn compact_jwt(claims: &Value) -> String {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims serialize"));
    format!("eyJhbGciOiJub25lIn0.{payload}.sig")
}

fn agent_presenter(iss: &str, sub: &str) -> VerifiedPresenter {
    VerifiedPresenter::Agent(VerifiedAgent {
        claims: serde_json::from_value(json!({
            "iss": iss, "sub": sub, "jti": "agent-token", "iat": 1, "exp": 2,
            "dwk": "aauth-agent.json",
            "cnf": {"jwk": {"kty": "EC", "crv": "P-256", "x": "x", "y": "y"}}
        }))
        .expect("agent claims"),
        jkt: "agent-thumbprint".into(),
        raw_jwt: "agent.jwt.raw".into(),
    })
}

#[test]
fn credential_subject_refuses_an_empty_issuer_or_subject() {
    assert_eq!(
        CredentialSubject::new("", "alice"),
        Err(CredentialSubjectError::EmptyIssuer)
    );
    assert_eq!(
        CredentialSubject::new("https://idp.test", ""),
        Err(CredentialSubjectError::EmptySubject)
    );
}

#[test]
fn credential_subjects_match_only_on_both_issuer_and_subject() {
    let alice = CredentialSubject::new("https://idp.test", "alice").expect("subject");
    assert_eq!(
        alice,
        CredentialSubject::new("https://idp.test", "alice").expect("subject")
    );
    assert_ne!(
        alice,
        CredentialSubject::new("https://idp.test/", "alice").expect("subject")
    );
    assert_ne!(
        alice,
        CredentialSubject::new("https://idp.test", "Alice").expect("subject")
    );
    assert_eq!(alice.to_string(), "alice (https://idp.test)");
}

#[test]
fn unverified_jwt_names_its_issuer_and_subject() {
    let token = compact_jwt(&json!({"iss": "https://idp.test", "sub": "alice", "aud": "bridge"}));
    let subject = CredentialSubject::from_unverified_jwt(&token).expect("subject");
    assert_eq!(subject.issuer(), "https://idp.test");
    assert_eq!(subject.subject(), "alice");
}

#[test]
fn unverified_jwt_refuses_anything_but_three_segments() {
    for token in ["opaque-token", "a.b", "a.b.c.d"] {
        assert!(
            matches!(
                CredentialSubject::from_unverified_jwt(token),
                Err(BearerSubjectError::NotCompactJwt)
            ),
            "{token}"
        );
    }
}

#[test]
fn unverified_jwt_refuses_a_payload_that_is_not_base64url() {
    assert!(matches!(
        CredentialSubject::from_unverified_jwt("h.e30=.s"),
        Err(BearerSubjectError::Encoding(_))
    ));
}

#[test]
fn unverified_jwt_refuses_missing_or_repeated_claims() {
    let missing = compact_jwt(&json!({"sub": "alice"}));
    assert!(matches!(
        CredentialSubject::from_unverified_jwt(&missing),
        Err(BearerSubjectError::Claims(_))
    ));
    for repeated in [
        br#"{"iss":"https://idp.test","iss":"https://attacker.test","sub":"alice"}"#.as_slice(),
        br#"{"iss":"https://idp.test","sub":"alice","sub":"mallory"}"#.as_slice(),
    ] {
        let repeated = format!("h.{}.s", URL_SAFE_NO_PAD.encode(repeated));
        assert!(matches!(
            CredentialSubject::from_unverified_jwt(&repeated),
            Err(BearerSubjectError::Claims(_))
        ));
    }
}

#[test]
fn unverified_jwt_refuses_an_empty_subject() {
    let token = compact_jwt(&json!({"iss": "https://idp.test", "sub": ""}));
    assert!(matches!(
        CredentialSubject::from_unverified_jwt(&token),
        Err(BearerSubjectError::Subject(CredentialSubjectError::EmptySubject))
    ));
}

#[test]
fn verified_caller_takes_the_agent_token_identity() {
    let caller =
        VerifiedEdgeCaller::from_presenter(&agent_presenter("https://agents.test", "agent-1")).expect("caller");
    assert_eq!(
        caller.subject(),
        &CredentialSubject::new("https://agents.test", "agent-1").expect("subject")
    );
    assert_eq!(caller.token(), PresentedToken::Agent);
    assert_eq!(caller.token().as_str(), "agent");
    assert_eq!(caller.key_thumbprint().as_str(), "agent-thumbprint");
}

#[test]
fn verified_caller_takes_the_auth_token_identity() {
    let presenter = VerifiedPresenter::Auth(VerifiedAuthPresenter {
        auth: VerifiedAuth {
            claims: serde_json::from_value(json!({
                "iss": "https://ps.test", "sub": "agent-1", "aud": "https://bridge.test",
                "jti": "grant", "iat": 1, "exp": 2, "agent": "agent-1",
                "agent_jkt": "auth-thumbprint", "scope": "message.send"
            }))
            .expect("auth claims"),
            raw_jwt: "auth.jwt.raw".into(),
        },
        jkt: "auth-thumbprint".into(),
    });
    let caller = VerifiedEdgeCaller::from_presenter(&presenter).expect("caller");
    assert_eq!(
        caller.subject(),
        &CredentialSubject::new("https://ps.test", "agent-1").expect("subject")
    );
    assert_eq!(caller.token(), PresentedToken::Auth);
    assert_eq!(caller.token().to_string(), "auth");
    assert_eq!(caller.key_thumbprint().to_string(), "auth-thumbprint");
}

#[test]
fn verified_caller_refuses_a_token_naming_no_subject() {
    assert_eq!(
        VerifiedEdgeCaller::from_presenter(&agent_presenter("https://agents.test", "")),
        Err(CredentialSubjectError::EmptySubject)
    );
}
