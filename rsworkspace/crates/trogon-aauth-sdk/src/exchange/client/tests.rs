#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use trogon_identity_types::aauth::person_server::TokenRequest;

fn fake_resource_token(iss: &str, agent: &str, agent_jkt: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(serde_json::json!({"alg": "ES256", "typ": "aa-resource+jwt"}).to_string());
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "iss": iss,
            "aud": "https://ps.example",
            "jti": "resource-jti-1",
            "iat": 1_700_000_000,
            "exp": 1_700_003_600,
            "dwk": "aauth-resource.json",
            "agent": agent,
            "agent_jkt": agent_jkt,
            "scope": "calendar.read",
        })
        .to_string(),
    );
    format!("{header}.{payload}.sig")
}

fn client_for(server: &MockServer) -> PsTokenClient<SignatureKeyOnlyHttpSigner> {
    let http = reqwest::Client::new();
    let signer = SignatureKeyOnlyHttpSigner::new("agent-jwt-value");
    PsTokenClient::new(http, signer, format!("{}/token", server.uri())).with_wait_secs(1)
}

#[tokio::test]
async fn grant_on_first_response_returns_granted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "auth_token": "eyJ.granted.token",
            "expires_in": 3600
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Granted(grant) => {
            assert_eq!(grant.auth_token, "eyJ.granted.token");
            assert_eq!(grant.expires_in, 3600);
        }
        other => panic!("expected Granted, got {other:?}"),
    }
}

#[tokio::test]
async fn signature_key_header_is_present_and_correctly_formatted_on_initial_post() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "auth_token": "eyJ.granted.token",
            "expires_in": 3600
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let _ = client.request_token(&request).await;

    let requests = server.received_requests().await.expect("request log");
    assert_eq!(requests.len(), 1);
    let sig_key = requests[0]
        .headers
        .get("signature-key")
        .expect("Signature-Key header present")
        .to_str()
        .expect("ascii header");
    assert_eq!(sig_key, "sig=jwt;jwt=\"agent-jwt-value\"");
}

#[tokio::test]
async fn pending_then_grant_polls_location_and_resolves() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/abc123")
                .insert_header("Retry-After", "0")
                .insert_header("Cache-Control", "no-store")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "auth_token": "eyJ.granted.token",
            "expires_in": 3600
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Granted(grant) => assert_eq!(grant.auth_token, "eyJ.granted.token"),
        other => panic!("expected Granted after polling, got {other:?}"),
    }
}

#[tokio::test]
async fn interaction_requirement_is_surfaced_distinctly_and_can_resume_polling() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/int1")
                .insert_header("Retry-After", "0")
                .insert_header("Cache-Control", "no-store")
                .insert_header(
                    "AAuth-Requirement",
                    r#"requirement=interaction; url="https://ps.example/interaction"; code="A1B2-C3D4""#,
                )
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/int1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "auth_token": "eyJ.granted.token",
            "expires_in": 3600
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    let poll_location = match outcome {
        ExchangeOutcome::Interaction {
            url,
            code,
            poll_location,
        } => {
            assert_eq!(url, "https://ps.example/interaction");
            assert_eq!(code.as_deref(), Some("A1B2-C3D4"));
            poll_location
        }
        other => panic!("expected Interaction, got {other:?}"),
    };

    let resumed = client.poll(&poll_location).await;
    match resumed {
        ExchangeOutcome::Granted(grant) => assert_eq!(grant.auth_token, "eyJ.granted.token"),
        other => panic!("expected Granted after resuming poll, got {other:?}"),
    }
}

#[tokio::test]
async fn clarification_question_is_surfaced_then_response_resumes_and_grants() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/clar1")
                .insert_header("Retry-After", "0")
                .insert_header("Cache-Control", "no-store")
                .set_body_json(serde_json::json!({
                    "status": "pending",
                    "clarification": "Why do you need calendar write access?",
                    "timeout": 120
                })),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/pending/clar1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "auth_token": "eyJ.granted.token",
            "expires_in": 3600
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    let poll_location = match outcome {
        ExchangeOutcome::Clarification {
            question,
            timeout,
            poll_location,
            ..
        } => {
            assert_eq!(question, "Why do you need calendar write access?");
            assert_eq!(timeout, Some(120));
            poll_location
        }
        other => panic!("expected Clarification, got {other:?}"),
    };

    let resumed = client
        .respond_to_clarification(&poll_location, "I need to create a shared event.")
        .await;
    match resumed {
        ExchangeOutcome::Granted(grant) => assert_eq!(grant.auth_token, "eyJ.granted.token"),
        other => panic!("expected Granted after clarification response, got {other:?}"),
    }
}

#[tokio::test]
async fn updated_request_is_rejected_client_side_when_resource_token_claims_mismatch() {
    let server = MockServer::start().await;
    // No mocks needed: the mismatch must be caught before any HTTP call.
    let client = client_for(&server);

    let original = fake_resource_token("https://calendar.example", "aauth:asst@agent.example", "jkt-1");
    let mismatched = fake_resource_token("https://other-resource.example", "aauth:asst@agent.example", "jkt-1");

    let outcome = client
        .submit_updated_request("/pending/clar1", &original, &mismatched, None)
        .await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::UpdatedRequestClaimsMismatch) => {}
        other => panic!("expected UpdatedRequestClaimsMismatch, got {other:?}"),
    }

    assert!(server.received_requests().await.expect("log").is_empty());
}

#[tokio::test]
async fn updated_request_is_accepted_and_sent_when_resource_token_claims_match() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/pending/clar1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "auth_token": "eyJ.granted.token",
            "expires_in": 3600
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let original = fake_resource_token("https://calendar.example", "aauth:asst@agent.example", "jkt-1");
    let updated = fake_resource_token("https://calendar.example", "aauth:asst@agent.example", "jkt-1");

    let outcome = client
        .submit_updated_request(
            &format!("{}/pending/clar1", server.uri()),
            &original,
            &updated,
            Some("reduced scope".to_string()),
        )
        .await;

    match outcome {
        ExchangeOutcome::Granted(grant) => assert_eq!(grant.auth_token, "eyJ.granted.token"),
        other => panic!("expected Granted, got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_deletes_pending_url_and_subsequent_poll_maps_410_to_gone() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/pending/cancel1"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/cancel1"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let location = format!("{}/pending/cancel1", server.uri());

    client.cancel(&location).await.expect("cancel should succeed");

    let outcome = client.poll(&location).await;
    match outcome {
        ExchangeOutcome::Denied(TerminalReason::Gone) => {}
        other => panic!("expected Denied(Gone) after cancel, got {other:?}"),
    }
}

#[tokio::test]
async fn token_endpoint_error_maps_to_typed_expired_resource_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "expired_resource_token",
            "detail": "The resource token expired."
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::TokenEndpoint(
            trogon_identity_types::aauth::error::TokenEndpointError::ExpiredResourceToken,
        )) => {}
        other => panic!("expected typed ExpiredResourceToken error, got {other:?}"),
    }
}
