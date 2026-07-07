use super::*;
use trogon_identity_types::aauth::AuthClaims;

fn verified_auth(iss: &str, aud: &str, act: Option<Act>) -> VerifiedAuth {
    VerifiedAuth {
        claims: AuthClaims {
            iss: iss.to_string(),
            sub: "user-123".to_string(),
            aud: aud.to_string(),
            jti: "jti-1".to_string(),
            iat: 0,
            exp: 1000,
            agent: "aauth:asst@agent.example".to_string(),
            agent_jkt: "thumbprint".to_string(),
            scope: "read".to_string(),
            principal: None,
            consent_id: None,
            resource: None,
            act,
            cnf: None,
        },
        raw_jwt: "raw.jwt.token".to_string(),
    }
}

#[test]
fn verify_upstream_token_accepts_trusted_issuer_and_bound_audience() {
    let upstream = verified_auth("https://as.example", "https://intermediary.example", None);
    let req = UpstreamTokenRequest {
        upstream: &upstream,
        intermediary_agent_token_iss: "https://intermediary.example",
        intermediary_agent_identifier: "aauth:booking@booking.example",
    };
    let result = verify_upstream_token(&req, |iss| iss == "https://as.example").expect("should verify");
    assert_eq!(result.downstream_act.agent, "aauth:booking@booking.example");
    assert!(result.downstream_act.act.is_none());
}

#[test]
fn verify_upstream_token_nests_existing_act_chain() {
    let inner = Act {
        agent: "aauth:asst@agent.example".to_string(),
        act: None,
    };
    let upstream = verified_auth(
        "https://as.example",
        "https://intermediary.example",
        Some(inner.clone()),
    );
    let req = UpstreamTokenRequest {
        upstream: &upstream,
        intermediary_agent_token_iss: "https://intermediary.example",
        intermediary_agent_identifier: "aauth:booking@booking.example",
    };
    let result = verify_upstream_token(&req, |_| true).expect("should verify");
    assert_eq!(result.downstream_act.agent, "aauth:booking@booking.example");
    assert_eq!(result.downstream_act.act.as_deref(), Some(&inner));
}

#[test]
fn verify_upstream_token_rejects_untrusted_issuer() {
    let upstream = verified_auth("https://untrusted.example", "https://intermediary.example", None);
    let req = UpstreamTokenRequest {
        upstream: &upstream,
        intermediary_agent_token_iss: "https://intermediary.example",
        intermediary_agent_identifier: "aauth:booking@booking.example",
    };
    let err = verify_upstream_token(&req, |_| false).unwrap_err();
    assert!(matches!(err, UpstreamTokenError::UntrustedIssuer(iss) if iss == "https://untrusted.example"));
}

#[test]
fn verify_upstream_token_rejects_audience_not_bound_to_intermediary() {
    let upstream = verified_auth("https://as.example", "https://someone-else.example", None);
    let req = UpstreamTokenRequest {
        upstream: &upstream,
        intermediary_agent_token_iss: "https://intermediary.example",
        intermediary_agent_identifier: "aauth:booking@booking.example",
    };
    let err = verify_upstream_token(&req, |_| true).unwrap_err();
    assert!(matches!(err, UpstreamTokenError::AudienceNotBoundToIntermediary { .. }));
}

#[test]
fn upstream_token_error_display_messages_are_distinct() {
    let cases = [
        format!("{}", UpstreamTokenError::UntrustedIssuer("x".into())),
        format!(
            "{}",
            UpstreamTokenError::AudienceNotBoundToIntermediary {
                upstream_aud: "a".into(),
                agent_token_iss: "b".into(),
            }
        ),
    ];
    assert_ne!(cases[0], cases[1]);
}
