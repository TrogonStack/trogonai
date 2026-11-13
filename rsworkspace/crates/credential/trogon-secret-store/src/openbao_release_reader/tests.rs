use reqwest::StatusCode;
use serde_json::{Value, json};
use trogon_credential_domain::{
    CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, CredentialVersion, SourceKind,
};

use super::*;
use crate::constants::RELEASE_REQUEST_TIMEOUT;
use crate::openbao_harness::{
    AuditedOpenBao, RELEASE_READ_POLICY, RELEASE_TOKEN_ROLE, ScriptedOpenBao, StepClock, approle, child_tokens, grant,
    refusal,
};
use crate::{OpenBaoSecretStoreConfigError, SecretStorePut, SecretStoreRevoke};

const LOGIN_TOKEN: &str = "fake-login-token";
const CHILD_TOKEN: &str = "fake-child-token";
const MATERIAL: &str = "fake-credential-material";

fn correlation_id() -> AuditCorrelationId {
    AuditCorrelationId::new("release-7f3a").unwrap()
}

fn credential(owner: &str) -> CredentialRef {
    let scope = CredentialScope::source(CredentialOwnerId::new(owner).unwrap(), SourceKind::Discord);
    CredentialRef::new(
        CredentialIdentity::derive(scope, CredentialKind::BotToken),
        CredentialVersion::new(1).unwrap(),
    )
}

fn active_metadata() -> (StatusCode, Value) {
    (
        StatusCode::OK,
        json!({ "data": { "current_version": 1, "versions": { "1": { "deletion_time": "", "destroyed": false } } } }),
    )
}

fn revoked_metadata() -> (StatusCode, Value) {
    (
        StatusCode::OK,
        json!({ "data": { "current_version": 1, "versions": { "1": { "deletion_time": "2020-01-01T00:00:00Z", "destroyed": false } } } }),
    )
}

fn material() -> (StatusCode, Value) {
    (StatusCode::OK, json!({ "data": { "data": { "value": MATERIAL } } }))
}

async fn scripted_reader(openbao: &ScriptedOpenBao) -> OpenBaoReleaseReader {
    OpenBaoReleaseReader::login_with_clock(
        openbao.address(),
        OpenBaoTransport::PlaintextAllowed,
        OpenBaoMount::default(),
        approle("fake-role-id", "fake-secret-id"),
        child_tokens(),
        StepClock::new().shared(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_refused_login_builds_no_reader() {
    let openbao = ScriptedOpenBao::start([refusal(StatusCode::BAD_REQUEST, "invalid role or secret ID")]).await;

    let error = OpenBaoReleaseReader::login(
        openbao.address(),
        OpenBaoTransport::PlaintextAllowed,
        OpenBaoMount::default(),
        approle("fake-role-id", "fake-secret-id"),
        child_tokens(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, OpenBaoLoginError::Refused { message } if message.contains("invalid role or secret ID")));
}

#[tokio::test]
async fn a_stalled_login_fails_within_the_release_call_bound() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let stall = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((connection, _)) = listener.accept().await {
            held.push(connection);
        }
    });
    let started = std::time::Instant::now();

    let error = OpenBaoReleaseReader::login(
        address,
        OpenBaoTransport::PlaintextAllowed,
        OpenBaoMount::default(),
        approle("fake-role-id", "fake-secret-id"),
        child_tokens(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, OpenBaoLoginError::Refused { .. }));
    assert!(started.elapsed() < RELEASE_REQUEST_TIMEOUT * 3);
    stall.abort();
}

#[tokio::test]
async fn a_plaintext_address_is_refused_before_any_login() {
    let openbao = ScriptedOpenBao::start([]).await;

    let error = OpenBaoReleaseReader::login(
        openbao.address(),
        OpenBaoTransport::TlsRequired,
        OpenBaoMount::default(),
        approle("fake-role-id", "fake-secret-id"),
        child_tokens(),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        OpenBaoLoginError::Config(OpenBaoSecretStoreConfigError::PlaintextAddress)
    ));
    assert!(openbao.calls().await.is_empty());
}

#[tokio::test]
async fn a_refused_child_token_fails_the_read_without_touching_the_credential() {
    let openbao = ScriptedOpenBao::start([grant(LOGIN_TOKEN, 60, true)]).await;
    let reader = scripted_reader(&openbao).await;
    openbao
        .answer_next([refusal(StatusCode::FORBIDDEN, "permission denied")])
        .await;

    let error = reader
        .get_correlated(&credential("tenant-1"), &correlation_id())
        .await
        .unwrap_err();

    assert!(matches!(error, SecretStoreError::BackendUnavailable { .. }));
    assert_eq!(openbao.paths().await.len(), 2);
}

#[tokio::test]
async fn a_retired_version_is_refused_before_its_material_is_read() {
    let openbao = ScriptedOpenBao::start([grant(LOGIN_TOKEN, 60, true)]).await;
    let reader = scripted_reader(&openbao).await;
    openbao
        .answer_next([grant(CHILD_TOKEN, 5, false), revoked_metadata()])
        .await;

    let error = reader
        .get_correlated(&credential("tenant-1"), &correlation_id())
        .await
        .unwrap_err();

    assert!(matches!(error, SecretStoreError::Unreadable { .. }));
    assert_eq!(openbao.paths().await.len(), 3);
}

#[tokio::test]
async fn each_read_mints_a_bounded_child_carrying_the_correlation_id() {
    let openbao = ScriptedOpenBao::start([grant(LOGIN_TOKEN, 60, true)]).await;
    let reader = scripted_reader(&openbao).await;
    openbao
        .answer_next([grant(CHILD_TOKEN, 5, false), active_metadata(), material()])
        .await;

    let read = reader
        .get_correlated(&credential("tenant-1"), &correlation_id())
        .await
        .unwrap();

    assert_eq!(read.as_plaintext().unwrap().as_str(), MATERIAL);
    let calls = openbao.calls().await;
    let mint = &calls[1];
    assert_eq!(mint.path, format!("/v1/auth/token/create/{RELEASE_TOKEN_ROLE}"));
    assert_eq!(mint.token.as_deref(), Some(LOGIN_TOKEN));
    assert_eq!(mint.correlation_id.as_deref(), Some("release-7f3a"));
    assert_eq!(
        mint.body,
        json!({
            "policies": [RELEASE_READ_POLICY],
            "no_default_policy": true,
            "ttl": "5s",
            "num_uses": 2,
            "renewable": false,
            "display_name": "credential-release",
            "meta": { "correlation_id": "release-7f3a" },
        })
    );
    for read in &calls[2..] {
        assert!(read.path.starts_with("/v1/secret/"));
        assert_eq!(read.token.as_deref(), Some(CHILD_TOKEN));
        assert_eq!(read.correlation_id.as_deref(), Some("release-7f3a"));
    }
    assert_eq!(calls.len(), 4);
}

#[tokio::test]
async fn an_empty_material_answer_fails_the_read_instead_of_releasing_nothing() {
    let openbao = ScriptedOpenBao::start([grant(LOGIN_TOKEN, 60, true)]).await;
    let reader = scripted_reader(&openbao).await;
    openbao
        .answer_next([
            grant(CHILD_TOKEN, 5, false),
            active_metadata(),
            (StatusCode::OK, json!({ "data": { "data": { "value": "" } } })),
        ])
        .await;

    let error = reader
        .get_correlated(&credential("tenant-1"), &correlation_id())
        .await
        .unwrap_err();

    assert!(matches!(error, SecretStoreError::BackendUnavailable { .. }));
    assert_eq!(openbao.paths().await.len(), 4);
}

#[tokio::test]
async fn debug_never_shows_a_token() {
    let openbao = ScriptedOpenBao::start([grant(LOGIN_TOKEN, 60, true)]).await;
    let reader = scripted_reader(&openbao).await;

    let debug = format!("{reader:?}");

    assert!(!debug.contains(LOGIN_TOKEN));
    assert!(!debug.contains("fake-secret-id"));
}

async fn seeded(openbao: &AuditedOpenBao, owner: &str) -> CredentialRef {
    let scope = CredentialScope::source(CredentialOwnerId::new(owner).unwrap(), SourceKind::Discord);
    openbao
        .root_store()
        .put(
            CredentialIdentity::derive(scope, CredentialKind::BotToken),
            SecretString::new(MATERIAL).unwrap(),
        )
        .await
        .unwrap()
}

async fn container_reader(openbao: &AuditedOpenBao) -> OpenBaoReleaseReader {
    OpenBaoReleaseReader::login(
        openbao.address(),
        OpenBaoTransport::PlaintextAllowed,
        OpenBaoMount::default(),
        openbao.approle().await,
        child_tokens(),
    )
    .await
    .unwrap()
}

fn kv_path(openbao: &AuditedOpenBao, credential: &CredentialRef) -> (String, String) {
    let store = openbao.root_store();
    (
        store.metadata_path(&credential.key()),
        format!("{}?version=1", store.data_path(&credential.key())),
    )
}

#[tokio::test]
async fn openbao_refuses_a_login_with_the_wrong_secret_id() {
    let openbao = AuditedOpenBao::start().await;
    let real = openbao.approle().await;

    let error = OpenBaoReleaseReader::login(
        openbao.address(),
        OpenBaoTransport::PlaintextAllowed,
        OpenBaoMount::default(),
        approle(real.role_id(), "fake-wrong-secret-id"),
        child_tokens(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, OpenBaoLoginError::Refused { .. }));
}

#[tokio::test]
async fn openbao_lets_the_login_token_read_no_credential_and_mint_nothing_else() {
    let openbao = AuditedOpenBao::start().await;
    let credential = seeded(&openbao, "tenant-1").await;
    let login = openbao.login_token(&openbao.approle().await).await;
    let (metadata, data) = kv_path(&openbao, &credential);

    let (metadata_status, _) = openbao.call(&login, "GET", &metadata, None).await;
    let (data_status, _) = openbao.call(&login, "GET", &data, None).await;
    let (direct_status, _) = openbao
        .call(
            &login,
            "POST",
            "auth/token/create",
            Some(json!({ "policies": [RELEASE_READ_POLICY] })),
        )
        .await;
    let (wider_status, _) = openbao.mint_child(&login, "root").await;

    assert_eq!(metadata_status, StatusCode::FORBIDDEN);
    assert_eq!(data_status, StatusCode::FORBIDDEN);
    assert_eq!(direct_status, StatusCode::FORBIDDEN);
    assert_eq!(wider_status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn openbao_scopes_a_child_to_the_credential_prefix_and_its_uses() {
    let openbao = AuditedOpenBao::start().await;
    let credential = seeded(&openbao, "tenant-1").await;
    let (root_status, _) = openbao
        .call(
            "dev-only-token",
            "POST",
            "secret/data/elsewhere/thing",
            Some(json!({ "data": { "value": "fake-outside-value" } })),
        )
        .await;
    assert_eq!(root_status, StatusCode::OK);
    let login = openbao.login_token(&openbao.approle().await).await;
    let (metadata, data) = kv_path(&openbao, &credential);

    let (_, outside_child) = openbao.mint_child(&login, RELEASE_READ_POLICY).await;
    let outside_child = outside_child["auth"]["client_token"].as_str().unwrap().to_string();
    let (outside_status, _) = openbao
        .call(&outside_child, "GET", "secret/data/elsewhere/thing", None)
        .await;

    let (minted, child) = openbao.mint_child(&login, RELEASE_READ_POLICY).await;
    assert_eq!(minted, StatusCode::OK);
    assert_eq!(child["auth"]["orphan"], false);
    let child = child["auth"]["client_token"].as_str().unwrap().to_string();
    let (metadata_status, _) = openbao.call(&child, "GET", &metadata, None).await;
    let (data_status, body) = openbao.call(&child, "GET", &data, None).await;
    let (third_status, _) = openbao.call(&child, "GET", &data, None).await;

    assert_eq!(outside_status, StatusCode::FORBIDDEN);
    assert_eq!(metadata_status, StatusCode::OK);
    assert_eq!(data_status, StatusCode::OK);
    assert_eq!(body["data"]["data"]["value"], MATERIAL);
    assert_eq!(third_status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn openbao_severs_a_child_when_its_login_token_is_revoked() {
    let openbao = AuditedOpenBao::start().await;
    let credential = seeded(&openbao, "tenant-1").await;
    let login = openbao.login_token(&openbao.approle().await).await;
    let (metadata, _) = kv_path(&openbao, &credential);
    let (_, child) = openbao.mint_child(&login, RELEASE_READ_POLICY).await;
    let child = child["auth"]["client_token"].as_str().unwrap().to_string();

    openbao.revoke(&login).await;
    let (status, _) = openbao.call(&child, "GET", &metadata, None).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn openbao_reader_refuses_a_revoked_credential() {
    let openbao = AuditedOpenBao::start().await;
    let credential = seeded(&openbao, "tenant-1").await;
    openbao.root_store().revoke(&credential).await.unwrap();
    let reader = container_reader(&openbao).await;

    let error = reader.get_correlated(&credential, &correlation_id()).await.unwrap_err();

    assert!(matches!(error, SecretStoreError::Unreadable { .. }));
}

#[tokio::test]
async fn openbao_reader_reads_the_precise_material() {
    let openbao = AuditedOpenBao::start().await;
    let credential = seeded(&openbao, "tenant-1").await;
    let reader = container_reader(&openbao).await;

    let read = reader.get_correlated(&credential, &correlation_id()).await.unwrap();
    let again = reader.get_correlated(&credential, &correlation_id()).await.unwrap();

    assert_eq!(read.as_plaintext().unwrap().as_str(), MATERIAL);
    assert_eq!(again.as_plaintext().unwrap().as_str(), MATERIAL);
}

#[tokio::test]
async fn openbao_audit_log_ties_the_read_to_the_release() {
    let openbao = AuditedOpenBao::start().await;
    let credential = seeded(&openbao, "tenant-1").await;
    let reader = container_reader(&openbao).await;
    let data_path = openbao.root_store().data_path(&credential.key());

    reader.get_correlated(&credential, &correlation_id()).await.unwrap();

    let log = openbao.audit_log().await;
    let read = log
        .iter()
        .find(|entry| {
            entry["type"] == "response"
                && entry["request"]["operation"] == "read"
                && entry["request"]["path"] == data_path.as_str()
        })
        .expect("an audit entry for the credential read");
    assert_eq!(read["auth"]["metadata"]["correlation_id"], "release-7f3a");
    assert_eq!(read["auth"]["display_name"], "token-credential-release");
    assert_eq!(
        read["request"]["headers"]["x-trogon-correlation-id"],
        json!(["release-7f3a"])
    );
    assert!(!log.iter().any(|entry| entry.to_string().contains(MATERIAL)));
}
