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
    PsTokenClient::new(http, signer, format!("{}/token", server.uri()))
        .with_wait_secs(1)
        .with_min_poll_interval_secs(0)
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

#[tokio::test]
async fn poll_budget_exhaustion_fails_cleanly_instead_of_looping_forever() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/p1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    // The pending URL never resolves: always 202 pending.
    Mock::given(method("GET"))
        .and(path("/pending/p1"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/p1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;

    let client = client_for(&server).with_max_poll_iterations(3);
    let request = TokenRequest::new("resource-token-value");
    let outcome = client.request_token(&request).await;
    match outcome {
        ExchangeOutcome::Error(ExchangeError::PollBudgetExhausted { polls }) => assert_eq!(polls, 4),
        other => panic!("expected PollBudgetExhausted, got {other:?}"),
    }
}

/// A loopback URI that nothing listens on: `TcpListener::bind` claims a free
/// port from the OS, then drops it immediately, so a connection attempt gets
/// an instant, deterministic "connection refused" without touching any real
/// network or external host.
fn unreachable_uri() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    format!("http://{addr}")
}

#[tokio::test]
async fn transport_error_on_initial_post_is_surfaced_when_server_is_unreachable() {
    let http = reqwest::Client::new();
    let signer = SignatureKeyOnlyHttpSigner::new("agent-jwt-value");
    let client = PsTokenClient::new(http, signer, format!("{}/token", unreachable_uri()))
        .with_wait_secs(1)
        .with_min_poll_interval_secs(0);

    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::Transport(_)) => {}
        other => panic!("expected Transport error, got {other:?}"),
    }
}

#[tokio::test]
async fn transport_error_on_poll_is_surfaced_when_server_is_unreachable() {
    let http = reqwest::Client::new();
    let signer = SignatureKeyOnlyHttpSigner::new("agent-jwt-value");
    let client = PsTokenClient::new(http, signer, format!("{}/token", unreachable_uri()))
        .with_wait_secs(1)
        .with_min_poll_interval_secs(0);

    let outcome = client.poll(&format!("{}/pending/unreachable", unreachable_uri())).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::Transport(_)) => {}
        other => panic!("expected Transport error, got {other:?}"),
    }
}

#[tokio::test]
async fn transport_error_on_clarification_response_is_surfaced_when_server_is_unreachable() {
    let http = reqwest::Client::new();
    let signer = SignatureKeyOnlyHttpSigner::new("agent-jwt-value");
    let client = PsTokenClient::new(http, signer, format!("{}/token", unreachable_uri()))
        .with_wait_secs(1)
        .with_min_poll_interval_secs(0);

    let outcome = client
        .respond_to_clarification(&format!("{}/pending/unreachable", unreachable_uri()), "answer")
        .await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::Transport(_)) => {}
        other => panic!("expected Transport error, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_interval_floor_sleeps_a_real_second_when_retry_after_is_nonzero() {
    // Every other pending/poll test in this file uses `Retry-After: 0` with
    // `with_min_poll_interval_secs(0)`, so `sleep_secs.max(min)` is always 0
    // and the loop's actual `tokio::time::sleep` call never runs. Exercise
    // that call for real with a 1s interval rather than mocking time away.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/slp1")
                .insert_header("Retry-After", "1")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/slp1"))
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
        other => panic!("expected Granted after a real 1s poll sleep, got {other:?}"),
    }
}

#[tokio::test]
async fn unauthorized_with_unrecognized_error_code_is_surfaced_as_unrecognized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "some_future_error_code",
            "detail": "not modeled yet"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::UnrecognizedTokenEndpointError(err)) => {
            assert_eq!(err.error, "some_future_error_code");
        }
        other => panic!("expected UnrecognizedTokenEndpointError, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_json_body_on_grant_is_surfaced_as_malformed_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::MalformedBody(_)) => {}
        other => panic!("expected MalformedBody, got {other:?}"),
    }
}

#[tokio::test]
async fn pending_response_missing_location_header_is_surfaced_as_missing_location() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::MissingLocation) => {}
        other => panic!("expected MissingLocation, got {other:?}"),
    }
}

#[tokio::test]
async fn payment_required_is_surfaced_as_pending_and_can_be_polled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(402)
                .insert_header("Location", "/pending/pay1")
                .set_body_string(""),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/pay1"))
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
        ExchangeOutcome::Pending { poll_location } => poll_location,
        other => panic!("expected Pending after PaymentRequired, got {other:?}"),
    };

    let resumed = client.poll(&poll_location).await;
    match resumed {
        ExchangeOutcome::Granted(grant) => assert_eq!(grant.auth_token, "eyJ.granted.token"),
        other => panic!("expected Granted after paying, got {other:?}"),
    }
}

#[tokio::test]
async fn denied_poll_with_recognized_code_maps_to_typed_polling_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/den1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/den1"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "error": "abandoned"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Denied(TerminalReason::Denied(
            trogon_identity_types::aauth::error::PollingError::Abandoned,
        )) => {}
        other => panic!("expected Denied(Abandoned), got {other:?}"),
    }
}

#[tokio::test]
async fn denied_poll_with_unrecognized_code_defaults_to_denied() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/den2")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/den2"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "error": "some_new_denial_reason"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Denied(TerminalReason::Denied(trogon_identity_types::aauth::error::PollingError::Denied)) => {}
        other => panic!("expected Denied(Denied) default, got {other:?}"),
    }
}

#[tokio::test]
async fn expired_poll_maps_to_denied_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/exp1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/exp1"))
        .respond_with(ResponseTemplate::new(408))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Denied(TerminalReason::Expired) => {}
        other => panic!("expected Denied(Expired), got {other:?}"),
    }
}

#[tokio::test]
async fn server_error_poll_maps_to_unexpected_status_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/err1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/err1"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::UnexpectedStatus(500)) => {}
        other => panic!("expected UnexpectedStatus(500), got {other:?}"),
    }
}

#[tokio::test]
async fn slow_down_poll_without_retry_after_falls_back_to_default_interval_then_grants() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/sd1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/sd1"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/sd1"))
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
        other => panic!("expected Granted after service-unavailable retry, got {other:?}"),
    }
}

#[tokio::test]
async fn unexpected_status_code_on_poll_is_surfaced_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/weird1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/weird1"))
        .respond_with(ResponseTemplate::new(418))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::UnexpectedStatus(418)) => {}
        other => panic!("expected UnexpectedStatus(418), got {other:?}"),
    }
}

#[tokio::test]
async fn relative_location_header_is_resolved_against_the_response_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/relative1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/relative1"))
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
        other => panic!("expected Granted via resolved relative Location, got {other:?}"),
    }
}

#[tokio::test]
async fn slow_down_on_poll_backs_off_then_grants_on_next_attempt() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "/pending/sd429")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/sd429"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pending/sd429"))
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
        other => panic!("expected Granted after a 429 slow-down retry, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_location_header_that_fails_to_resolve_is_used_verbatim_and_then_fails_the_next_poll() {
    // A `Location` value that cannot be joined against the response's own URL
    // (e.g. an invalid IPv6 host literal) drives `resolve_location`'s error
    // fallback: the raw header value is carried through as-is rather than
    // failing outright. Since it has no clarification/interaction
    // requirement, the core asks the driver to keep polling that (unusable)
    // location, so the loop's own follow-up GET is what ultimately surfaces
    // a Transport error building a request against the malformed URL.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", "http://[::1")
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::Transport(_)) => {}
        other => panic!("expected Transport error from polling the unresolved Location, got {other:?}"),
    }
}

#[tokio::test]
async fn transport_error_on_follow_up_poll_inside_the_loop_is_surfaced() {
    // The initial POST succeeds and hands back a Location on a host nothing
    // listens on, so the loop's own follow-up GET (not the initial POST) is
    // what hits the Transport error branch.
    let server = MockServer::start().await;
    let dead_location = format!("{}/pending/dead", unreachable_uri());
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("Location", dead_location.as_str())
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({"status": "pending"})),
        )
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::Transport(_)) => {}
        other => panic!("expected Transport error from the loop's follow-up poll, got {other:?}"),
    }
}

#[tokio::test]
async fn service_unavailable_on_initial_post_with_no_location_yet_is_surfaced_as_unexpected_status() {
    // A 503 on the very first POST (before any Location exists to poll)
    // leaves the core in `AwaitingInitial` with a `Stop` action; the driver
    // has nothing to poll, so it surfaces this as a 503 UnexpectedStatus for
    // the caller to retry the initial request from scratch.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let request = TokenRequest::new("resource-token");
    let outcome = client.request_token(&request).await;

    match outcome {
        ExchangeOutcome::Error(ExchangeError::UnexpectedStatus(503)) => {}
        other => panic!("expected UnexpectedStatus(503), got {other:?}"),
    }
}
