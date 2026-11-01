use async_nats::jetstream::kv;
use async_nats::jetstream::stream::StorageType;
use axum::http::header::HOST;
use axum::http::{HeaderName, HeaderValue, Method};
use bytes::Bytes;
use trogon_aauth_verify::constants::AAUTH_REPLAY_BUCKET;
use trogon_aauth_verify::{
    ChallengeKey, ChallengeKid, FreshnessError, HttpPopError, InMemoryReplayStore, NatsKvReplayStore, NonceRequirement,
    NonceTtl, ReplayBucketSpec, ReplayError, ServerNonces, SystemTimeSource,
};
use trogon_identity_types::aauth::headers::SIGNATURE;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::edge_identity::{CredentialSubject, PresentedToken};
use crate::edge_signing_harness::{
    CHALLENGE_KID, EDGE_AUTHORITY, EDGE_RESOURCE, FailingReplayStore, REQUEST_PATH, Signing, SigningAgent,
    challenge_key_pem, now, post, unsigned,
};

fn verifier(agent: &SigningAgent, replay: Arc<dyn ReplayStore>) -> EdgePopVerifier {
    let jwks: Arc<dyn JwksResolver> = Arc::new(agent.jwks());
    let clock: Arc<dyn TimeSource> = Arc::new(SystemTimeSource);
    HttpPopVerifier::new(jwks, clock, replay, EDGE_RESOURCE)
}

fn authority() -> EdgeAuthority {
    EdgeAuthority::new(EDGE_AUTHORITY).unwrap()
}

fn guard(agent: &SigningAgent, policy: ProofPolicy) -> PopEdgeGuard {
    guard_over(agent, policy, Arc::new(InMemoryReplayStore::default()))
}

fn guard_over(agent: &SigningAgent, policy: ProofPolicy, replay: Arc<dyn ReplayStore>) -> PopEdgeGuard {
    PopEdgeGuard::new(verifier(agent, replay), authority(), policy)
}

fn nonce_guard(agent: &SigningAgent) -> PopEdgeGuard {
    let key = ChallengeKey::from_pkcs8_pem(&challenge_key_pem(), ChallengeKid::new(CHALLENGE_KID).unwrap()).unwrap();
    let nonces = ServerNonces::new(key, NonceTtl::default(), NonceRequirement::Required);
    PopEdgeGuard::new(
        verifier(agent, Arc::new(InMemoryReplayStore::default())).with_server_nonces(nonces),
        authority(),
        ProofPolicy::Enforce,
    )
}

/// A request signed by `agent` whose `Signature` was made by another key, so
/// it does not verify against the key the agent token confirms.
fn forged(agent: &SigningAgent) -> InboundRequest {
    let signing = Signing::fresh();
    let mut headers = agent.signature_headers(Method::POST.as_str(), REQUEST_PATH, b"{}", &signing);
    let forger = SigningAgent::new().signature_headers(Method::POST.as_str(), REQUEST_PATH, b"{}", &signing);
    headers.insert(
        HeaderName::from_bytes(SIGNATURE.as_bytes()).unwrap(),
        forger.get(SIGNATURE).unwrap().clone(),
    );
    post(headers, Bytes::from_static(b"{}"))
}

async fn refusal(guard: &PopEdgeGuard, request: &InboundRequest, caller: &CallerHttpsAuth) -> EdgeAuthError {
    guard.admit(request, caller).await.unwrap_err()
}

#[tokio::test]
async fn enforce_admits_a_request_signed_by_the_caller_its_bearer_names() {
    let agent = SigningAgent::new();
    let admission = guard(&agent, ProofPolicy::Enforce)
        .admit(&agent.signed(b"{}", &Signing::fresh()), &agent.bearer())
        .await
        .unwrap();

    let EdgeAdmission::Verified(caller) = admission else {
        panic!("expected a verified caller, got {admission:?}");
    };
    assert_eq!(
        caller.subject(),
        &CredentialSubject::new(agent.issuer(), agent.subject()).unwrap()
    );
    assert_eq!(caller.token(), PresentedToken::Agent);
    assert!(!caller.key_thumbprint().as_str().is_empty());
}

#[tokio::test]
async fn enforce_refuses_an_unsigned_request() {
    let agent = SigningAgent::new();
    let error = refusal(&guard(&agent, ProofPolicy::Enforce), &unsigned(b"{}"), &agent.bearer()).await;
    assert!(matches!(error, EdgeAuthError::Unsigned(_)), "got {error:?}");
}

#[tokio::test]
async fn enforce_refuses_a_signature_the_confirmed_key_did_not_make() {
    let agent = SigningAgent::new();
    let error = refusal(&guard(&agent, ProofPolicy::Enforce), &forged(&agent), &agent.bearer()).await;
    assert!(
        matches!(error, EdgeAuthError::Unverified(HttpPopError::BadSignature)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn enforce_refuses_a_body_other_than_the_one_signed() {
    let agent = SigningAgent::new();
    let headers = agent.signature_headers(Method::POST.as_str(), REQUEST_PATH, b"{}", &Signing::fresh());
    let request = post(headers, Bytes::from_static(b"{\"tampered\":true}"));
    let error = refusal(&guard(&agent, ProofPolicy::Enforce), &request, &agent.bearer()).await;
    assert!(
        matches!(error, EdgeAuthError::Unverified(HttpPopError::ContentDigestMismatch)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn enforce_refuses_an_expired_signature() {
    let agent = SigningAgent::new();
    let signing = Signing::fresh().window(now() - 120, now() - 90);
    let error = refusal(
        &guard(&agent, ProofPolicy::Enforce),
        &agent.signed(b"{}", &signing),
        &agent.bearer(),
    )
    .await;
    assert!(
        matches!(
            error,
            EdgeAuthError::Unverified(HttpPopError::Freshness(FreshnessError::Expired))
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn enforce_refuses_a_signature_created_further_ahead_than_the_tolerated_skew() {
    let agent = SigningAgent::new();
    let signing = Signing::fresh().window(now() + 45, now() + 75);
    let error = refusal(
        &guard(&agent, ProofPolicy::Enforce),
        &agent.signed(b"{}", &signing),
        &agent.bearer(),
    )
    .await;
    assert!(
        matches!(
            error,
            EdgeAuthError::Unverified(HttpPopError::Freshness(FreshnessError::NotYetValid))
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn enforce_refuses_a_signature_valid_for_longer_than_the_edge_accepts() {
    let agent = SigningAgent::new();
    let signing = Signing::fresh().window(now(), now() + 90);
    let error = refusal(
        &guard(&agent, ProofPolicy::Enforce),
        &agent.signed(b"{}", &signing),
        &agent.bearer(),
    )
    .await;
    assert!(
        matches!(
            error,
            EdgeAuthError::Unverified(HttpPopError::Freshness(FreshnessError::InvalidWindow))
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn enforce_refuses_a_jti_its_key_already_presented_even_under_a_new_window() {
    let agent = SigningAgent::new();
    let guard = guard(&agent, ProofPolicy::Enforce);
    let first = Signing::fresh().jti("edge-replayed-jti");
    guard
        .admit(&agent.signed(b"{}", &first), &agent.bearer())
        .await
        .unwrap();

    let again = Signing::fresh().window(now() - 5, now() + 30).jti("edge-replayed-jti");
    let error = refusal(&guard, &agent.signed(b"{}", &again), &agent.bearer()).await;
    assert!(
        matches!(error, EdgeAuthError::Unverified(HttpPopError::Replay)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn enforce_refuses_a_proof_for_another_caller_than_the_bearer_names() {
    let agent = SigningAgent::new();
    let other = SigningAgent::named(agent.issuer(), "aauth:agent-2@agents.test");
    let error = refusal(
        &guard(&agent, ProofPolicy::Enforce),
        &agent.signed(b"{}", &Signing::fresh()),
        &other.bearer(),
    )
    .await;

    let EdgeAuthError::BearerMismatch { verified, bearer } = error else {
        panic!("expected a bearer mismatch, got {error:?}");
    };
    assert_eq!(verified.subject(), agent.subject());
    assert_eq!(bearer.subject(), other.subject());
}

#[tokio::test]
async fn enforce_refuses_a_bearer_whose_issuer_differs_from_the_verified_one() {
    let agent = SigningAgent::new();
    let elsewhere = SigningAgent::named("https://elsewhere.test", agent.subject());
    let error = refusal(
        &guard(&agent, ProofPolicy::Enforce),
        &agent.signed(b"{}", &Signing::fresh()),
        &elsewhere.bearer(),
    )
    .await;
    assert!(matches!(error, EdgeAuthError::BearerMismatch { .. }), "got {error:?}");
}

#[tokio::test]
async fn enforce_refuses_a_bearer_that_names_no_caller() {
    let agent = SigningAgent::new();
    let error = refusal(
        &guard(&agent, ProofPolicy::Enforce),
        &agent.signed(b"{}", &Signing::fresh()),
        &CallerHttpsAuth::new("Bearer opaque-token"),
    )
    .await;
    assert!(matches!(error, EdgeAuthError::UnidentifiedBearer(_)), "got {error:?}");
}

#[tokio::test]
async fn a_bearer_naming_no_caller_leaves_the_jti_unspent() {
    let agent = SigningAgent::new();
    let guard = guard(&agent, ProofPolicy::Enforce);
    let request = agent.signed(b"{}", &Signing::fresh());
    let error = refusal(&guard, &request, &CallerHttpsAuth::new("Bearer opaque-token")).await;
    assert!(matches!(error, EdgeAuthError::UnidentifiedBearer(_)), "got {error:?}");

    let admission = guard.admit(&request, &agent.bearer()).await.unwrap();
    assert!(matches!(admission, EdgeAdmission::Verified(_)), "got {admission:?}");
}

#[tokio::test]
async fn enforce_fails_closed_when_the_replay_store_cannot_answer() {
    let agent = SigningAgent::new();
    let error = refusal(
        &guard_over(&agent, ProofPolicy::Enforce, Arc::new(FailingReplayStore)),
        &agent.signed(b"{}", &Signing::fresh()),
        &agent.bearer(),
    )
    .await;
    assert!(
        matches!(error, EdgeAuthError::Unavailable(HttpPopError::Backend(_))),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_request_signed_for_another_authority_does_not_verify() {
    let agent = SigningAgent::new();
    let signing = Signing::fresh().authority("elsewhere.test");
    let error = refusal(
        &guard(&agent, ProofPolicy::Enforce),
        &agent.signed(b"{}", &signing),
        &agent.bearer(),
    )
    .await;
    assert!(
        matches!(error, EdgeAuthError::Unverified(HttpPopError::BadSignature)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn the_host_a_request_claims_does_not_choose_the_authority_it_is_verified_for() {
    let agent = SigningAgent::new();
    let guard = guard(&agent, ProofPolicy::Enforce);
    let mut headers = agent.signature_headers(Method::POST.as_str(), REQUEST_PATH, b"{}", &Signing::fresh());
    headers.insert(HOST, HeaderValue::from_static("elsewhere.test"));
    let admission = guard
        .admit(&post(headers, Bytes::from_static(b"{}")), &agent.bearer())
        .await
        .unwrap();
    assert!(matches!(admission, EdgeAdmission::Verified(_)), "got {admission:?}");

    let elsewhere = Signing::fresh().authority("elsewhere.test");
    let mut headers = agent.signature_headers(Method::POST.as_str(), REQUEST_PATH, b"{}", &elsewhere);
    headers.insert(HOST, HeaderValue::from_static("elsewhere.test"));
    let error = refusal(&guard, &post(headers, Bytes::from_static(b"{}")), &agent.bearer()).await;
    assert!(matches!(error, EdgeAuthError::Unverified(_)), "got {error:?}");
}

#[tokio::test]
async fn a_required_nonce_is_handed_out_and_admits_the_retry_that_binds_it() {
    let agent = SigningAgent::new();
    let guard = nonce_guard(&agent);
    let error = refusal(&guard, &agent.signed(b"{}", &Signing::fresh()), &agent.bearer()).await;
    let EdgeAuthError::NonceRequired(challenge) = error else {
        panic!("expected a nonce challenge, got {error:?}");
    };
    assert!(challenge.refused().is_none());
    assert_eq!(
        EdgeAuthError::NonceRequired(challenge).detail(),
        "request has to bind a server nonce issued for it"
    );

    let error = refusal(&guard, &agent.signed(b"{}", &Signing::fresh()), &agent.bearer()).await;
    let EdgeAuthError::NonceRequired(challenge) = error else {
        panic!("expected a nonce challenge, got {error:?}");
    };

    let retry = Signing::fresh().server_nonce(challenge.nonce().as_str());
    let admission = guard
        .admit(&agent.signed(b"{}", &retry), &agent.bearer())
        .await
        .unwrap();
    assert!(matches!(admission, EdgeAdmission::Verified(_)), "got {admission:?}");
}

#[tokio::test]
async fn a_nonce_the_edge_never_issued_is_answered_with_one_it_did() {
    let agent = SigningAgent::new();
    let elsewhere = nonce_guard(&agent);
    let error = refusal(&elsewhere, &agent.signed(b"{}", &Signing::fresh()), &agent.bearer()).await;
    let EdgeAuthError::NonceRequired(challenge) = error else {
        panic!("expected a nonce challenge, got {error:?}");
    };
    let signing = Signing::fresh().server_nonce(challenge.nonce().as_str());
    let error = refusal(&nonce_guard(&agent), &agent.signed(b"{}", &signing), &agent.bearer()).await;
    let EdgeAuthError::NonceRequired(challenge) = error else {
        panic!("expected a nonce challenge, got {error:?}");
    };
    assert!(challenge.refused().is_some());
    let detail = EdgeAuthError::NonceRequired(challenge).detail();
    assert!(detail.starts_with("request has to bind a server nonce issued for it: "));
}

#[tokio::test]
async fn shadow_still_refuses_an_unsigned_request() {
    let agent = SigningAgent::new();
    let error = refusal(&guard(&agent, ProofPolicy::Shadow), &unsigned(b"{}"), &agent.bearer()).await;
    assert!(matches!(error, EdgeAuthError::Unsigned(_)), "got {error:?}");
}

#[tokio::test]
async fn shadow_still_refuses_a_bearer_that_names_no_caller() {
    let agent = SigningAgent::new();
    let error = refusal(
        &guard(&agent, ProofPolicy::Shadow),
        &agent.signed(b"{}", &Signing::fresh()),
        &CallerHttpsAuth::new("Bearer opaque-token"),
    )
    .await;
    assert!(matches!(error, EdgeAuthError::UnidentifiedBearer(_)), "got {error:?}");
}

#[tokio::test]
async fn shadow_admits_a_proof_that_fails_and_keeps_why() {
    let agent = SigningAgent::new();
    let admission = guard(&agent, ProofPolicy::Shadow)
        .admit(&forged(&agent), &agent.bearer())
        .await
        .unwrap();
    assert!(
        matches!(
            admission,
            EdgeAdmission::Shadowed(EdgeAuthError::Unverified(HttpPopError::BadSignature))
        ),
        "got {admission:?}"
    );
}

#[tokio::test]
async fn shadow_admits_a_mismatched_bearer_and_keeps_why() {
    let agent = SigningAgent::new();
    let other = SigningAgent::named(agent.issuer(), "aauth:agent-2@agents.test");
    let admission = guard(&agent, ProofPolicy::Shadow)
        .admit(&agent.signed(b"{}", &Signing::fresh()), &other.bearer())
        .await
        .unwrap();
    assert!(
        matches!(admission, EdgeAdmission::Shadowed(EdgeAuthError::BearerMismatch { .. })),
        "got {admission:?}"
    );
}

#[tokio::test]
async fn shadow_reports_a_proof_that_verifies_as_verified() {
    let agent = SigningAgent::new();
    let guard = guard(&agent, ProofPolicy::Shadow);
    let admission = guard
        .admit(&agent.signed(b"{}", &Signing::fresh()), &agent.bearer())
        .await
        .unwrap();
    assert!(matches!(admission, EdgeAdmission::Verified(_)), "got {admission:?}");
    assert_eq!(guard.mode(), EdgeAuthMode::Shadow);
    assert_eq!(guard.policy(), ProofPolicy::Shadow);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_one_edge_instance_admitted_replays_through_no_other() {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    jetstream
        .create_key_value(kv::Config {
            bucket: AAUTH_REPLAY_BUCKET.to_owned(),
            history: ReplayBucketSpec::REQUIRED.history(),
            max_age: ReplayBucketSpec::REQUIRED.max_age(),
            storage: StorageType::File,
            ..Default::default()
        })
        .await
        .unwrap();
    let agent = SigningAgent::new();
    let first = guard_over(
        &agent,
        ProofPolicy::Enforce,
        Arc::new(NatsKvReplayStore::open(&jetstream).await.unwrap()),
    );
    let second = guard_over(
        &agent,
        ProofPolicy::Enforce,
        Arc::new(NatsKvReplayStore::open(&jetstream).await.unwrap()),
    );
    let request = agent.signed(b"{}", &Signing::fresh());

    let admission = first.admit(&request, &agent.bearer()).await.unwrap();
    assert!(matches!(admission, EdgeAdmission::Verified(_)), "got {admission:?}");
    let error = refusal(&second, &request, &agent.bearer()).await;
    assert!(
        matches!(error, EdgeAuthError::Unverified(HttpPopError::Replay)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn bearer_only_guard_admits_every_request_unchecked() {
    let admission = BearerOnlyGuard
        .admit(&unsigned(b"{}"), &CallerHttpsAuth::new("Bearer opaque-token"))
        .await
        .unwrap();
    assert!(matches!(admission, EdgeAdmission::Unchecked), "got {admission:?}");
    assert_eq!(BearerOnlyGuard.mode(), EdgeAuthMode::Off);
}

#[test]
fn an_authority_is_a_lowercased_host_and_optional_port() {
    let authority = EdgeAuthority::new(" Bridge.Test:8443 ").unwrap();
    assert_eq!(authority.as_str(), "bridge.test:8443");
    assert_eq!(authority.to_string(), "bridge.test:8443");
}

#[test]
fn an_authority_carries_nothing_beyond_a_host_and_port() {
    for raw in [
        "https://bridge.test",
        "bridge.test/a2a",
        "agent@bridge.test",
        "bridge.test?x=1",
        "bridge.test#a",
        "bridge test",
    ] {
        assert!(
            matches!(EdgeAuthority::new(raw), Err(EdgeAuthorityError::NotAnAuthority(_))),
            "{raw} was accepted"
        );
    }
    assert_eq!(EdgeAuthority::new("  "), Err(EdgeAuthorityError::Empty));
}

#[test]
fn a_resource_identifier_is_never_empty() {
    assert_eq!(ResourceIdentifier::new(" "), Err(EmptyResourceIdentifierError));
    assert_eq!(ResourceIdentifier::new(EDGE_RESOURCE).unwrap().as_str(), EDGE_RESOURCE);
    assert_eq!(
        ResourceIdentifier::new(EDGE_RESOURCE).unwrap().to_string(),
        EDGE_RESOURCE
    );
}

#[test]
fn a_guard_debug_value_names_its_authority_and_policy() {
    let agent = SigningAgent::new();
    let rendered = format!("{:?}", guard(&agent, ProofPolicy::Shadow));
    assert!(rendered.contains(EDGE_AUTHORITY));
    assert!(rendered.contains("Shadow"));
}

#[tokio::test]
async fn a_failed_replay_store_fails_closed_when_asked_about_a_record() {
    assert!(matches!(
        FailingReplayStore.contains("jti.key.record").await,
        Err(ReplayError::MutexPoisoned)
    ));
}

#[test]
fn modes_read_as_the_values_they_are_configured_with() {
    assert_eq!(EdgeAuthMode::Enforce.to_string(), "enforce");
    assert_eq!(EdgeAuthMode::Shadow.to_string(), "shadow");
    assert_eq!(EdgeAuthMode::Off.to_string(), "off");
    assert_eq!(ProofPolicy::Enforce.to_string(), "enforce");
    assert_eq!(ProofPolicy::Shadow.mode(), EdgeAuthMode::Shadow);
}
