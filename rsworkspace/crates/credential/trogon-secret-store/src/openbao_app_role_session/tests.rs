use std::time::Duration;

use reqwest::StatusCode;

use super::*;
use crate::constants::{RENEWAL_WINDOW_DIVISOR, TOKEN_EXPIRY_MARGIN};
use crate::openbao_harness::{ScriptedOpenBao, StepClock, approle, grant, refusal};

const LOGIN_PATH: &str = "/v1/auth/approle/login";
const RENEW_PATH: &str = "/v1/auth/token/renew-self";
const FIRST_TOKEN: &str = "fake-login-token-1";
const SECOND_TOKEN: &str = "fake-login-token-2";
const TTL_SECS: u64 = 60;

fn fake_approle() -> OpenBaoAppRole {
    approle("fake-role-id", "fake-secret-id")
}

async fn started(openbao: &ScriptedOpenBao, clock: &StepClock) -> OpenBaoAppRoleSession {
    OpenBaoAppRoleSession::start(fake_approle(), clock.shared(), &reqwest::Client::new(), &openbao.url())
        .await
        .unwrap()
}

async fn token(session: &OpenBaoAppRoleSession, openbao: &ScriptedOpenBao) -> Result<String, SecretStoreError> {
    session
        .token(&reqwest::Client::new(), &openbao.url())
        .await
        .map(|token| token.as_str().to_string())
}

/// Moves the clock into the renewal window but leaves the token usable.
fn into_renewal_window(clock: &StepClock) {
    clock.advance(Duration::from_secs(
        TTL_SECS - TTL_SECS / u64::from(RENEWAL_WINDOW_DIVISOR),
    ));
}

#[tokio::test]
async fn a_refused_login_refuses_to_start() {
    let openbao = ScriptedOpenBao::start([refusal(StatusCode::BAD_REQUEST, "invalid role or secret ID")]).await;

    let error = OpenBaoAppRoleSession::start(
        fake_approle(),
        StepClock::new().shared(),
        &reqwest::Client::new(),
        &openbao.url(),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        OpenBaoCallError::Status {
            status: StatusCode::BAD_REQUEST,
            ..
        }
    ));
    assert_eq!(openbao.paths().await, [LOGIN_PATH]);
}

#[tokio::test]
async fn a_login_sends_the_role_and_secret_id_without_a_token() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TTL_SECS, true)]).await;

    started(&openbao, &StepClock::new()).await;

    let calls = openbao.calls().await;
    assert_eq!(calls[0].token, None);
    assert_eq!(calls[0].body["role_id"], "fake-role-id");
    assert_eq!(calls[0].body["secret_id"], "fake-secret-id");
}

#[tokio::test]
async fn a_login_granting_less_than_the_expiry_margin_refuses_to_start() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TOKEN_EXPIRY_MARGIN.as_secs(), true)]).await;

    let error = OpenBaoAppRoleSession::start(
        fake_approle(),
        StepClock::new().shared(),
        &reqwest::Client::new(),
        &openbao.url(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, OpenBaoCallError::UnusableLease { .. }));
}

#[tokio::test]
async fn a_malformed_login_answer_never_quotes_the_body() {
    let openbao = ScriptedOpenBao::start([(
        StatusCode::OK,
        serde_json::json!({ "auth": { "client_token": "fake-quoted-token", "lease_duration": "soon" } }),
    )])
    .await;

    let error = OpenBaoAppRoleSession::start(
        fake_approle(),
        StepClock::new().shared(),
        &reqwest::Client::new(),
        &openbao.url(),
    )
    .await
    .unwrap_err();

    assert!(!error.to_string().contains("fake-quoted-token"));
}

#[tokio::test]
async fn a_renewal_refused_falls_back_to_a_fresh_login() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TTL_SECS, true)]).await;
    let clock = StepClock::new();
    let session = started(&openbao, &clock).await;
    openbao
        .answer_next([
            refusal(StatusCode::FORBIDDEN, "permission denied"),
            grant(SECOND_TOKEN, TTL_SECS, true),
        ])
        .await;
    into_renewal_window(&clock);

    assert_eq!(token(&session, &openbao).await.unwrap(), SECOND_TOKEN);
    assert_eq!(openbao.paths().await, [LOGIN_PATH, RENEW_PATH, LOGIN_PATH]);
    assert_eq!(openbao.calls().await[1].token.as_deref(), Some(FIRST_TOKEN));
}

#[tokio::test]
async fn a_renewal_capped_by_the_max_ttl_falls_back_to_a_fresh_login() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TTL_SECS, true)]).await;
    let clock = StepClock::new();
    let session = started(&openbao, &clock).await;
    openbao
        .answer_next([
            grant(FIRST_TOKEN, TTL_SECS / 2, true),
            grant(SECOND_TOKEN, TTL_SECS, true),
        ])
        .await;
    into_renewal_window(&clock);

    assert_eq!(token(&session, &openbao).await.unwrap(), SECOND_TOKEN);
    assert_eq!(openbao.paths().await, [LOGIN_PATH, RENEW_PATH, LOGIN_PATH]);
}

#[tokio::test]
async fn a_token_past_its_usable_life_is_never_sent() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TTL_SECS, true)]).await;
    let clock = StepClock::new();
    let session = started(&openbao, &clock).await;
    openbao
        .answer_next([refusal(StatusCode::SERVICE_UNAVAILABLE, "sealed")])
        .await;
    clock.advance(Duration::from_secs(TTL_SECS) - TOKEN_EXPIRY_MARGIN);

    let error = token(&session, &openbao).await.unwrap_err();

    assert!(matches!(error, SecretStoreError::BackendUnavailable { .. }));
    assert_eq!(openbao.paths().await, [LOGIN_PATH, LOGIN_PATH]);
    assert!(
        openbao
            .calls()
            .await
            .iter()
            .all(|call| call.token.as_deref() != Some(FIRST_TOKEN))
    );
}

#[tokio::test]
async fn a_failed_refresh_keeps_failing_closed_until_a_login_succeeds() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TTL_SECS, true)]).await;
    let clock = StepClock::new();
    let session = started(&openbao, &clock).await;
    openbao
        .answer_next([
            refusal(StatusCode::FORBIDDEN, "permission denied"),
            refusal(StatusCode::SERVICE_UNAVAILABLE, "sealed"),
        ])
        .await;
    into_renewal_window(&clock);

    assert!(token(&session, &openbao).await.is_err());

    openbao.answer_next([grant(SECOND_TOKEN, TTL_SECS, true)]).await;
    assert_eq!(token(&session, &openbao).await.unwrap(), SECOND_TOKEN);
    assert_eq!(openbao.paths().await, [LOGIN_PATH, RENEW_PATH, LOGIN_PATH, LOGIN_PATH]);
}

#[tokio::test]
async fn a_fixed_lease_is_replaced_by_a_login_instead_of_renewed() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TTL_SECS, false)]).await;
    let clock = StepClock::new();
    let session = started(&openbao, &clock).await;
    openbao.answer_next([grant(SECOND_TOKEN, TTL_SECS, false)]).await;
    into_renewal_window(&clock);

    assert_eq!(token(&session, &openbao).await.unwrap(), SECOND_TOKEN);
    assert_eq!(openbao.paths().await, [LOGIN_PATH, LOGIN_PATH]);
}

#[tokio::test]
async fn a_token_outside_the_renewal_window_is_used_without_a_call() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TTL_SECS, true)]).await;
    let clock = StepClock::new();
    let session = started(&openbao, &clock).await;
    clock.advance(Duration::from_secs(1));

    assert_eq!(token(&session, &openbao).await.unwrap(), FIRST_TOKEN);
    assert_eq!(openbao.paths().await, [LOGIN_PATH]);
}

#[tokio::test]
async fn a_token_in_the_renewal_window_is_renewed_for_the_login_ttl() {
    let openbao = ScriptedOpenBao::start([grant(FIRST_TOKEN, TTL_SECS, true)]).await;
    let clock = StepClock::new();
    let session = started(&openbao, &clock).await;
    openbao.answer_next([grant(FIRST_TOKEN, TTL_SECS, true)]).await;
    into_renewal_window(&clock);

    assert_eq!(token(&session, &openbao).await.unwrap(), FIRST_TOKEN);
    assert_eq!(openbao.paths().await, [LOGIN_PATH, RENEW_PATH]);
    assert_eq!(openbao.calls().await[1].body["increment"], format!("{TTL_SECS}s"));

    clock.advance(Duration::from_secs(1));
    assert_eq!(token(&session, &openbao).await.unwrap(), FIRST_TOKEN);
    assert_eq!(openbao.paths().await.len(), 2);
}

#[test]
fn debug_never_shows_the_secret_id() {
    let session = OpenBaoAppRoleSession {
        approle: fake_approle(),
        clock: StepClock::new().shared(),
        lease: Mutex::new(None),
    };

    assert!(!format!("{session:?}").contains("fake-secret-id"));
}
