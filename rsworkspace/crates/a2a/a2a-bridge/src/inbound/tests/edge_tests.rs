use std::sync::Mutex;

use axum::http::HeaderName;
use axum::response::Response;
use trogon_aauth_verify::{InMemoryReplayStore, ReplayStore};

use super::*;
use crate::constants::{ENV_AAUTH_DEV_UNENFORCED, ENV_AAUTH_MODE};
use crate::edge_auth::EdgeAuthConfig;
use crate::edge_signing_harness::{
    EdgeEnv, FailingReplayStore, REQUEST_PATH, Signing, SigningAgent, now, with_authorization,
};

/// Mints the fixture token, remembering every bearer it was asked to mint for.
#[derive(Default)]
struct RecordingMint {
    minted_for: Mutex<Vec<String>>,
}

impl RecordingMint {
    fn minted_for(&self) -> Vec<String> {
        self.minted_for.lock().unwrap().clone()
    }
}

#[async_trait]
impl AuthCalloutClient for RecordingMint {
    async fn mint(&self, caller_auth: &CallerHttpsAuth) -> Result<BridgeUserJwt, BridgeError> {
        self.minted_for.lock().unwrap().push(caller_auth.as_str().to_owned());
        StubAuthCalloutMint::fixture()?.mint(caller_auth).await
    }
}

/// The bridge behind an edge guard, with what it minted and forwarded.
struct Bridge {
    state: AppState,
    mint: Arc<RecordingMint>,
    publisher: Arc<RecordingInboundPublisher>,
}

impl Bridge {
    fn behind(guard: Arc<dyn EdgeGuard>) -> Self {
        let mint = Arc::new(RecordingMint::default());
        let publisher = Arc::new(RecordingInboundPublisher::new());
        let state = AppState::new(
            mint.clone(),
            publisher.clone(),
            Arc::new(EmptyTaskJetStream),
            default_a2a_prefix(),
            guard,
        );
        Self { state, mint, publisher }
    }

    async fn refuses(&self, request: InboundRequest) -> Response {
        let error = handle_jsonrpc(request, &self.state).await.unwrap_err();
        assert!(
            matches!(error, BridgeError::EdgeAuth(_) | BridgeError::MissingAuthorization),
            "got {error:?}"
        );
        assert!(self.mint.minted_for().is_empty(), "minted for a refused request");
        assert!(self.publisher.peek_subject().is_none(), "forwarded a refused request");
        bridge_error_into_response(error)
    }
}

fn guard_from(edge: &EdgeEnv, replay: Arc<dyn ReplayStore>) -> Arc<dyn EdgeGuard> {
    let EdgeAuthConfig::Proof(settings) = EdgeAuthConfig::from_env(edge.env(), edge.fs()).unwrap() else {
        panic!("expected settings that verify proofs");
    };
    Arc::new(settings.guard(replay))
}

fn enforcing(agent: &SigningAgent) -> Arc<dyn EdgeGuard> {
    guard_from(&EdgeEnv::trusting(agent), Arc::new(InMemoryReplayStore::default()))
}

fn shadowing(agent: &SigningAgent) -> Arc<dyn EdgeGuard> {
    let edge = EdgeEnv::trusting(agent)
        .set(ENV_AAUTH_MODE, "shadow")
        .set(ENV_AAUTH_DEV_UNENFORCED, "true");
    guard_from(&edge, Arc::new(InMemoryReplayStore::default()))
}

fn tasks_get() -> &'static [u8] {
    br#"{"jsonrpc":"2.0","id":1,"method":"tasks/get","params":{"id":"task-1"}}"#
}

fn to_planner(headers: HeaderMap, authorization: &str) -> InboundRequest {
    let mut headers = with_authorization(headers, authorization);
    headers.insert(
        HeaderName::from_static(AGENT_ID_HEADER),
        HeaderValue::from_static("planner"),
    );
    post(headers, Bytes::from_static(tasks_get()))
}

/// A `tasks/get` for agent `planner`, signed by `agent` under `signing` and
/// carrying `authorization`.
fn signed_rpc(agent: &SigningAgent, signing: &Signing, authorization: &str) -> InboundRequest {
    let headers = agent.signature_headers(Method::POST.as_str(), REQUEST_PATH, tasks_get(), signing);
    to_planner(headers, authorization)
}

fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn a_verified_request_is_minted_for_its_own_bearer_and_forwarded() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));
    let bearer = agent.bearer();

    let response = handle_jsonrpc(signed_rpc(&agent, &Signing::fresh(), bearer.as_str()), &bridge.state)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(bridge.mint.minted_for(), vec![bearer.as_str().to_owned()]);
    assert_eq!(
        bridge.publisher.peek_subject().as_deref(),
        Some("a2a.v1.gateway.planner.person.harness.tasks.get")
    );
}

#[tokio::test]
async fn an_unsigned_request_is_told_to_sign_with_an_agent_token() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));

    let response = bridge
        .refuses(to_planner(HeaderMap::new(), agent.bearer().as_str()))
        .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        header(&response, REQUIREMENT),
        Some(Requirement::AgentToken.to_header_value().as_str())
    );
}

#[tokio::test]
async fn an_expired_signature_is_told_to_sign_again() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));
    let signing = Signing::fresh().window(now() - 120, now() - 90);

    let response = bridge
        .refuses(signed_rpc(&agent, &signing, agent.bearer().as_str()))
        .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        header(&response, REQUIREMENT),
        Some(Requirement::AgentToken.to_header_value().as_str())
    );
}

#[tokio::test]
async fn a_replayed_request_is_minted_for_only_once() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));
    let signing = Signing::fresh();
    let bearer = agent.bearer();

    let first = handle_jsonrpc(signed_rpc(&agent, &signing, bearer.as_str()), &bridge.state)
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let error = handle_jsonrpc(signed_rpc(&agent, &signing, bearer.as_str()), &bridge.state)
        .await
        .unwrap_err();
    assert!(
        matches!(error, BridgeError::EdgeAuth(EdgeAuthError::Unverified(_))),
        "got {error:?}"
    );
    assert_eq!(bridge_error_into_response(error).status(), StatusCode::UNAUTHORIZED);
    assert_eq!(bridge.mint.minted_for().len(), 1);
}

#[tokio::test]
async fn a_bearer_naming_another_caller_than_the_signer_is_an_invalid_token() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));
    let other = SigningAgent::named(agent.issuer(), "aauth:agent-2@agents.test");

    let response = bridge
        .refuses(signed_rpc(&agent, &Signing::fresh(), other.bearer().as_str()))
        .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        header(&response, WWW_AUTHENTICATE.as_str()),
        Some(WWW_AUTHENTICATE_INVALID_BEARER)
    );
}

#[tokio::test]
async fn a_bearer_naming_no_caller_is_an_invalid_token() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));

    let response = bridge
        .refuses(signed_rpc(&agent, &Signing::fresh(), "Bearer opaque-token"))
        .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        header(&response, WWW_AUTHENTICATE.as_str()),
        Some(WWW_AUTHENTICATE_INVALID_BEARER)
    );
}

#[tokio::test]
async fn a_request_without_a_bearer_is_challenged_for_one() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));
    let headers = agent.signature_headers(Method::POST.as_str(), REQUEST_PATH, tasks_get(), &Signing::fresh());

    let response = bridge.refuses(post(headers, Bytes::from_static(tasks_get()))).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        header(&response, WWW_AUTHENTICATE.as_str()),
        Some(WWW_AUTHENTICATE_BEARER)
    );
}

#[tokio::test]
async fn an_edge_that_cannot_check_proofs_is_unavailable_rather_than_open() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(guard_from(&EdgeEnv::trusting(&agent), Arc::new(FailingReplayStore)));

    let response = bridge
        .refuses(signed_rpc(&agent, &Signing::fresh(), agent.bearer().as_str()))
        .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_required_nonce_is_handed_to_the_caller_and_admits_its_retry() {
    let agent = SigningAgent::new();
    let edge = EdgeEnv::trusting(&agent).requiring_nonces();
    let bridge = Bridge::behind(guard_from(&edge, Arc::new(InMemoryReplayStore::default())));
    let bearer = agent.bearer();

    let challenge = bridge
        .refuses(signed_rpc(&agent, &Signing::fresh(), bearer.as_str()))
        .await;
    assert_eq!(challenge.status(), StatusCode::UNAUTHORIZED);
    let nonce = header(&challenge, SERVER_NONCE_HEADER).unwrap().to_owned();

    let retry = Signing::fresh().server_nonce(&nonce);
    let response = handle_jsonrpc(signed_rpc(&agent, &retry, bearer.as_str()), &bridge.state)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(bridge.mint.minted_for(), vec![bearer.as_str().to_owned()]);
}

#[tokio::test]
async fn shadow_refuses_an_unsigned_request_before_minting_for_it() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(shadowing(&agent));

    let response = bridge
        .refuses(to_planner(HeaderMap::new(), agent.bearer().as_str()))
        .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn shadow_refuses_a_bearer_naming_no_caller_before_minting_for_it() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(shadowing(&agent));

    let response = bridge
        .refuses(signed_rpc(&agent, &Signing::fresh(), "Bearer opaque-token"))
        .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn shadow_forwards_a_proof_that_fails() {
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(shadowing(&agent));
    let other = SigningAgent::named(agent.issuer(), "aauth:agent-2@agents.test");
    let bearer = other.bearer();

    let response = handle_jsonrpc(signed_rpc(&agent, &Signing::fresh(), bearer.as_str()), &bridge.state)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(bridge.mint.minted_for(), vec![bearer.as_str().to_owned()]);
}

/// How a record names the caller the edge verified for `agent`.
fn verified_as(agent: &SigningAgent) -> String {
    format!("{} ({})", agent.subject(), agent.issuer())
}

#[tokio::test]
async fn a_verified_request_is_recorded_against_the_minted_caller_and_the_bridge() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));

    handle_jsonrpc(
        signed_rpc(&agent, &Signing::fresh(), agent.bearer().as_str()),
        &bridge.state,
    )
    .await
    .unwrap();

    let records = logs.records();
    let presented = records
        .iter()
        .find(|log| log.message.contains("presenting the request to the gateway"));
    assert!(
        presented.is_some_and(
            |log| log.level == LogLevel::Info && records_request(log, "person.harness", "subject_principal", "1")
        ),
        "{:?}",
        messages(&records)
    );
    assert!(
        !records.iter().any(|log| log.level == LogLevel::Warn),
        "{:?}",
        messages(&records)
    );
}

#[tokio::test]
async fn a_refusal_is_recorded_against_whoever_the_edge_verified_and_the_bridge() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(enforcing(&agent));
    let other = SigningAgent::named(agent.issuer(), "aauth:agent-2@agents.test");

    bridge
        .refuses(to_planner(HeaderMap::new(), agent.bearer().as_str()))
        .await;
    bridge
        .refuses(signed_rpc(&agent, &Signing::fresh(), other.bearer().as_str()))
        .await;

    let records = logs.records();
    let refusals: Vec<&CapturedLog> = records
        .iter()
        .filter(|log| log.message.contains("refused the request at the HTTP edge"))
        .collect();
    assert_eq!(refusals.len(), 2, "{:?}", messages(&records));
    assert!(
        records_request(refusals[0], "unverified", "unverified", "1"),
        "{}",
        refusals[0].message
    );
    assert!(
        records_request(refusals[1], &verified_as(&agent), "edge_identity", "1"),
        "{}",
        refusals[1].message
    );
    assert!(
        refusals
            .iter()
            .all(|log| log.level == LogLevel::Warn && log.message.contains("mode=enforce")),
        "{:?}",
        messages(&records)
    );
}

#[tokio::test]
async fn a_proof_shadow_mode_lets_through_is_recorded_against_its_signer_and_then_the_minted_caller() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let agent = SigningAgent::new();
    let bridge = Bridge::behind(shadowing(&agent));
    let other = SigningAgent::named(agent.issuer(), "aauth:agent-2@agents.test");

    handle_jsonrpc(
        signed_rpc(&agent, &Signing::fresh(), other.bearer().as_str()),
        &bridge.state,
    )
    .await
    .unwrap();

    let records = logs.records();
    let shadowed = records
        .iter()
        .find(|log| log.message.contains("shadow mode admitted the request"));
    assert!(
        shadowed.is_some_and(|log| log.level == LogLevel::Warn
            && records_request(log, &verified_as(&agent), "edge_identity", "1")
            && log.message.contains("mode=shadow")),
        "{:?}",
        messages(&records)
    );
    let presented = records
        .iter()
        .find(|log| log.message.contains("presenting the request to the gateway"));
    assert!(
        presented.is_some_and(|log| records_request(log, "person.harness", "subject_principal", "1")),
        "{:?}",
        messages(&records)
    );
}
