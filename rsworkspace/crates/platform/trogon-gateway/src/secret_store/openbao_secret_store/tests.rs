use std::net::SocketAddr;

use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};
use crate::source_integration_id::SourceIntegrationId;
use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use trogon_std::env::{ReadEnv, SystemEnv};

use super::*;

const OPENBAO_IMAGE: &str = "openbao/openbao";
const OPENBAO_IMAGE_TAG: &str = "2.5.5";
const OPENBAO_DEV_ROOT_TOKEN: &str = "dev-only-token";
const OPENBAO_PORT: u16 = 8200;
const TEST_INPUT: &str = "copy-this-value-in-and-out";

#[test]
fn encodes_openbao_path_segments() {
    assert_eq!(
        encode_path_segment("openbao:tenant-1:github/primary:webhook_secret"),
        "openbao%3Atenant-1%3Agithub%2Fprimary%3Awebhook_secret"
    );
}

#[test]
fn default_mount_is_secret() {
    assert_eq!(OpenBaoMount::default().as_str(), "secret");
}

#[test]
fn parses_integration_scoped_openbao_credential_id() {
    let id = CredentialId::new("openbao:tenant-1:github/primary:webhook_secret").unwrap();

    let credential = openbao_credential_ref_from_id(id, CredentialVersion::new(2).unwrap()).unwrap();

    assert_eq!(
        credential.id().as_str(),
        "openbao:tenant-1:github/primary:webhook_secret"
    );
    assert_eq!(credential.version().get(), 2);
    assert_eq!(credential.owner_id().as_str(), "tenant-1");
    assert_eq!(credential.source(), SourceKind::GitHub);
    assert_eq!(credential.scope_key(), "github/primary");
    assert_eq!(credential.kind(), CredentialKind::WebhookSecret);
}

#[test]
fn parses_source_scoped_openbao_credential_id() {
    let id = CredentialId::new("openbao:tenant-1:discord:bot_token").unwrap();

    let credential = openbao_credential_ref_from_id(id, CredentialVersion::initial()).unwrap();

    assert_eq!(credential.owner_id().as_str(), "tenant-1");
    assert_eq!(credential.source(), SourceKind::Discord);
    assert_eq!(credential.scope_key(), "discord");
    assert_eq!(credential.kind(), CredentialKind::BotToken);
}

#[test]
fn parses_datadog_openbao_credential_id() {
    let id = CredentialId::new("openbao:tenant-1:datadog/primary:webhook_token").unwrap();

    let credential = openbao_credential_ref_from_id(id, CredentialVersion::initial()).unwrap();

    assert_eq!(credential.source(), SourceKind::Datadog);
    assert_eq!(credential.scope_key(), "datadog/primary");
    assert_eq!(credential.kind(), CredentialKind::WebhookToken);
}

#[test]
fn parses_owner_id_with_colons_from_the_right() {
    let id = CredentialId::new("openbao:tenant:region:github/primary:webhook_secret").unwrap();

    let credential = openbao_credential_ref_from_id(id, CredentialVersion::initial()).unwrap();

    assert_eq!(credential.owner_id().as_str(), "tenant:region");
    assert_eq!(credential.scope_key(), "github/primary");
    assert_eq!(credential.kind(), CredentialKind::WebhookSecret);
}

#[test]
fn rejects_unknown_openbao_scope_source() {
    let id = CredentialId::new("openbao:tenant-1:unknown/primary:webhook_secret").unwrap();

    let error = openbao_credential_ref_from_id(id, CredentialVersion::initial()).unwrap_err();

    assert!(matches!(
        error,
        OpenBaoCredentialIdParseError::InvalidScope {
            source: OpenBaoScopeKeyParseError::UnknownSource,
            ..
        }
    ));
}

#[test]
fn rotated_credential_ref_preserves_integration_scope() {
    let scope = CredentialScope::integration(
        CredentialOwnerId::new("tenant-1").unwrap(),
        SourceKind::GitHub,
        SourceIntegrationId::new("primary").unwrap(),
    );
    let credential = CredentialRef::new(
        CredentialId::new("openbao:tenant-1:github/primary:webhook_secret").unwrap(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::WebhookSecret,
    );

    let rotated = credential.with_version(CredentialVersion::new(2).unwrap());

    assert_eq!(rotated.scope_key(), "github/primary");
    assert_eq!(rotated.id(), credential.id());
    assert_eq!(rotated.kind(), CredentialKind::WebhookSecret);
    assert_eq!(rotated.version().get(), 2);
}

#[derive(Clone)]
struct FakeOpenBao {
    address: SocketAddr,
    metadata: Option<Value>,
    metadata_failure: Option<(StatusCode, &'static str)>,
    write_response: Option<Value>,
    write_failure: Option<(StatusCode, &'static str)>,
    requests: Arc<Mutex<Vec<FakeOpenBaoRequest>>>,
}

struct FakeOpenBaoRequest {
    method: Method,
    path: String,
    body: Value,
}

impl FakeOpenBao {
    async fn start(metadata: Option<Value>) -> Self {
        Self::start_with(metadata, None, None, None).await
    }

    async fn start_answering_writes_with(write_response: Value) -> Self {
        Self::start_with(None, None, Some(write_response), None).await
    }

    async fn start_failing_writes(metadata: Value, status: StatusCode, body: &'static str) -> Self {
        Self::start_with(Some(metadata), None, None, Some((status, body))).await
    }

    async fn start_failing_metadata(status: StatusCode, body: &'static str) -> Self {
        Self::start_with(None, Some((status, body)), None, None).await
    }

    async fn start_with(
        metadata: Option<Value>,
        metadata_failure: Option<(StatusCode, &'static str)>,
        write_response: Option<Value>,
        write_failure: Option<(StatusCode, &'static str)>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let openbao = Self {
            address: listener.local_addr().unwrap(),
            metadata,
            metadata_failure,
            write_response,
            write_failure,
            requests: Arc::default(),
        };
        let app = Router::new().fallback(answer_as_openbao).with_state(openbao.clone());
        tokio::spawn(axum::serve(listener, app).into_future());
        openbao
    }

    fn store(&self) -> OpenBaoSecretStore {
        OpenBaoSecretStore::new(format!("http://{}", self.address), OPENBAO_DEV_ROOT_TOKEN).unwrap()
    }

    async fn posted_to(&self, endpoint: OpenBaoEndpoint) -> Vec<Value> {
        let prefix = format!("/v1/secret/{}/", endpoint.as_str());
        self.requests
            .lock()
            .await
            .iter()
            .filter(|request| request.method == Method::POST && request.path.starts_with(&prefix))
            .map(|request| request.body.clone())
            .collect()
    }
}

async fn answer_as_openbao(State(openbao): State<FakeOpenBao>, method: Method, uri: Uri, body: Bytes) -> Response {
    let response = match (&method, &openbao.metadata) {
        (&Method::GET, _) if openbao.metadata_failure.is_some() => {
            let (status, body) = openbao.metadata_failure.unwrap();
            (status, body).into_response()
        }
        (&Method::GET, Some(metadata)) => Json(json!({ "data": metadata })).into_response(),
        (&Method::GET, None) => (StatusCode::NOT_FOUND, Json(json!({ "errors": [] }))).into_response(),
        _ if openbao.write_failure.is_some() => {
            let (status, body) = openbao.write_failure.unwrap();
            (status, body).into_response()
        }
        _ if openbao.write_response.is_some() && uri.path().starts_with("/v1/secret/data/") => {
            Json(json!({ "data": openbao.write_response.clone().unwrap() })).into_response()
        }
        _ => StatusCode::NO_CONTENT.into_response(),
    };
    openbao.requests.lock().await.push(FakeOpenBaoRequest {
        method,
        path: uri.path().to_string(),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    response
}

fn discord_bot_token(version: CredentialVersion) -> CredentialRef {
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
    let id = openbao_credential_id(&scope, CredentialKind::BotToken).unwrap();
    CredentialRef::new(id, version, &scope, CredentialKind::BotToken)
}

#[tokio::test]
async fn destroy_of_unknown_credential_returns_missing() {
    let openbao = FakeOpenBao::start(None).await;
    let credential = discord_bot_token(CredentialVersion::initial());

    let error = openbao
        .store()
        .destroy(&credential, &SecretDestroyReason::new("operator request").unwrap())
        .await
        .unwrap_err();

    assert!(matches!(error, SecretStoreError::Missing { credential: missing } if missing == credential));
    assert!(openbao.posted_to(OpenBaoEndpoint::Destroy).await.is_empty());
}

#[tokio::test]
async fn metadata_failure_mentioning_404_is_a_backend_outage() {
    let openbao = FakeOpenBao::start_failing_metadata(StatusCode::INTERNAL_SERVER_ERROR, "upstream 404 on path").await;
    let credential = discord_bot_token(CredentialVersion::initial());

    let error = openbao.store().metadata(&credential).await.unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            ..
        }
    ));
}

#[tokio::test]
async fn metadata_of_unknown_credential_is_missing() {
    let openbao = FakeOpenBao::start(None).await;
    let credential = discord_bot_token(CredentialVersion::initial());

    let error = openbao.store().metadata(&credential).await.unwrap_err();

    assert!(matches!(error, SecretStoreError::Missing { credential: missing } if missing == credential));
}

fn metadata_with_destroyed_first_version() -> Value {
    json!({
        "current_version": 2,
        "versions": {
            "1": { "deletion_time": "", "destroyed": true },
            "2": { "deletion_time": "", "destroyed": false },
        },
    })
}

#[tokio::test]
async fn revoke_through_destroyed_version_is_unwritable_without_posting_delete() {
    let openbao = FakeOpenBao::start(Some(metadata_with_destroyed_first_version())).await;
    let credential = discord_bot_token(CredentialVersion::initial());

    let error = openbao.store().revoke(&credential).await.unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::Unwritable { credential: unwritable, status: CredentialStatus::Destroyed }
            if unwritable == credential
    ));
    assert!(openbao.posted_to(OpenBaoEndpoint::Delete).await.is_empty());
}

#[tokio::test]
async fn revoke_of_version_absent_from_metadata_is_missing_without_posting_delete() {
    let openbao = FakeOpenBao::start(Some(metadata_with_destroyed_first_version())).await;
    let credential = discord_bot_token(CredentialVersion::new(3).unwrap());

    let error = openbao.store().revoke(&credential).await.unwrap_err();

    assert!(matches!(error, SecretStoreError::Missing { credential: missing } if missing == credential));
    assert!(openbao.posted_to(OpenBaoEndpoint::Delete).await.is_empty());
}

#[tokio::test]
async fn revoke_posts_delete_for_every_version() {
    let openbao = FakeOpenBao::start(Some(metadata_with_destroyed_first_version())).await;

    openbao
        .store()
        .revoke(&discord_bot_token(CredentialVersion::new(2).unwrap()))
        .await
        .unwrap();

    assert_eq!(
        openbao.posted_to(OpenBaoEndpoint::Delete).await,
        vec![json!({ "versions": [1, 2] })]
    );
}

#[tokio::test]
async fn metadata_fingerprints_the_metadata_path_and_version() {
    let openbao = FakeOpenBao::start(Some(json!({
        "current_version": 2,
        "versions": { "2": { "deletion_time": "", "destroyed": false } },
    })))
    .await;
    let scope = CredentialScope::integration(
        CredentialOwnerId::new("tenant-compose-smoke").unwrap(),
        SourceKind::GitHub,
        SourceIntegrationId::new("compose-smoke").unwrap(),
    );
    let credential = CredentialRef::new(
        openbao_credential_id(&scope, CredentialKind::WebhookSecret).unwrap(),
        CredentialVersion::new(2).unwrap(),
        &scope,
        CredentialKind::WebhookSecret,
    );

    let metadata = openbao.store().metadata(&credential).await.unwrap();

    assert_eq!(
        metadata.fingerprint().as_str(),
        "openbao:ca45fc4dbbb75097b66c4fdf179938511f07d24c06be4a794ecc31841a61b1c4"
    );
}

#[test]
fn mount_accepts_its_allowed_alphabet() {
    let mount = OpenBaoMount::new("kv-v2_team.a").unwrap();

    assert_eq!(mount.as_str(), "kv-v2_team.a");
    assert_eq!(mount.to_string(), "kv-v2_team.a");
}

#[test]
fn mount_rejects_empty_value() {
    assert_eq!(OpenBaoMount::new(""), Err(OpenBaoMountError::Empty));
}

#[test]
fn mount_rejects_path_separators() {
    assert_eq!(OpenBaoMount::new("a/b"), Err(OpenBaoMountError::InvalidCharacter('/')));
}

#[test]
fn debug_redacts_the_token() {
    let store = OpenBaoSecretStore::new("http://openbao.example:8200", "s3cr3t-root-token").unwrap();

    let debug = format!("{store:?}");

    assert!(debug.contains("OpenBaoSecretStore"));
    assert!(debug.contains("openbao.example"));
    assert!(debug.contains("***"));
    assert!(!debug.contains("s3cr3t-root-token"));
}

#[test]
fn rejects_credential_id_without_the_openbao_prefix() {
    let id = CredentialId::new("tenant-1:discord:bot_token").unwrap();

    let error = openbao_credential_ref_from_id(id, CredentialVersion::initial()).unwrap_err();

    assert!(matches!(error, OpenBaoCredentialIdParseError::MissingPrefix { .. }));
}

#[test]
fn rejects_credential_id_without_owner_scope_and_kind() {
    let id = CredentialId::new("openbao:discord").unwrap();

    let error = openbao_credential_ref_from_id(id, CredentialVersion::initial()).unwrap_err();

    assert!(matches!(error, OpenBaoCredentialIdParseError::InvalidShape { .. }));
}

#[test]
fn rejects_scope_that_is_neither_a_source_nor_an_integration() {
    let id = CredentialId::new("openbao:tenant-1:primary:webhook_secret").unwrap();

    let error = openbao_credential_ref_from_id(id, CredentialVersion::initial()).unwrap_err();

    assert!(matches!(
        error,
        OpenBaoCredentialIdParseError::InvalidScope {
            source: OpenBaoScopeKeyParseError::MissingSource,
            ..
        }
    ));
}

#[tokio::test]
async fn address_that_cannot_be_a_base_is_a_backend_outage() {
    let store = OpenBaoSecretStore::new("mailto:ops@example.com", OPENBAO_DEV_ROOT_TOKEN).unwrap();

    let error = store
        .metadata(&discord_bot_token(CredentialVersion::initial()))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::BackendUnavailable { backend: StorageBackend::OpenBao, message }
            if message.starts_with("failed to build endpoint url")
    ));
}

#[tokio::test]
async fn put_with_an_unparseable_write_response_is_a_backend_outage() {
    let openbao = FakeOpenBao::start(None).await;
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);

    let error = openbao
        .store()
        .put(scope, CredentialKind::BotToken, SecretString::new("Bot token").unwrap())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            ..
        }
    ));
}

#[tokio::test]
async fn put_rejects_a_zero_version_from_the_backend() {
    let openbao = FakeOpenBao::start_answering_writes_with(json!({ "version": 0 })).await;
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);

    let error = openbao
        .store()
        .put(scope, CredentialKind::BotToken, SecretString::new("Bot token").unwrap())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            ..
        }
    ));
}

#[tokio::test]
async fn rotate_of_a_destroyed_version_is_unwritable() {
    let openbao = FakeOpenBao::start(Some(metadata_with_destroyed_first_version())).await;
    let credential = discord_bot_token(CredentialVersion::initial());

    let error = openbao
        .store()
        .rotate(&credential, SecretString::new("Bot new-token").unwrap())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::Unwritable { credential: unwritable, status: CredentialStatus::Destroyed }
            if unwritable == credential
    ));
    assert!(openbao.posted_to(OpenBaoEndpoint::Data).await.is_empty());
}

fn metadata_with_previous_and_revoked_versions() -> Value {
    json!({
        "current_version": 3,
        "versions": {
            "1": { "deletion_time": "", "destroyed": false },
            "2": { "deletion_time": "2026-01-01T00:00:00Z", "destroyed": false },
            "3": { "deletion_time": "", "destroyed": false },
        },
    })
}

#[tokio::test]
async fn rotate_of_a_previous_version_reports_previous() {
    let openbao = FakeOpenBao::start(Some(metadata_with_previous_and_revoked_versions())).await;
    let credential = discord_bot_token(CredentialVersion::initial());

    let error = openbao
        .store()
        .rotate(&credential, SecretString::new("Bot new-token").unwrap())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::Unwritable {
            status: CredentialStatus::Previous,
            ..
        }
    ));
}

#[tokio::test]
async fn metadata_maps_a_deleted_version_to_revoked() {
    let openbao = FakeOpenBao::start(Some(metadata_with_previous_and_revoked_versions())).await;

    let metadata = openbao
        .store()
        .metadata(&discord_bot_token(CredentialVersion::new(2).unwrap()))
        .await
        .unwrap();

    assert_eq!(metadata.status(), CredentialStatus::Revoked);
}

#[tokio::test]
async fn get_of_a_revoked_version_is_unreadable() {
    let openbao = FakeOpenBao::start(Some(metadata_with_previous_and_revoked_versions())).await;

    let error = openbao
        .store()
        .get(&discord_bot_token(CredentialVersion::new(2).unwrap()))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::Unreadable {
            status: CredentialStatus::Revoked,
            ..
        }
    ));
}

#[tokio::test]
async fn metadata_of_a_version_absent_from_the_backend_is_missing() {
    let openbao = FakeOpenBao::start(Some(metadata_with_previous_and_revoked_versions())).await;
    let credential = discord_bot_token(CredentialVersion::new(9).unwrap());

    let error = openbao.store().metadata(&credential).await.unwrap_err();

    assert!(matches!(error, SecretStoreError::Missing { credential: missing } if missing == credential));
}

#[tokio::test]
async fn revoke_reports_a_backend_that_rejects_the_delete() {
    let openbao = FakeOpenBao::start_failing_writes(
        metadata_with_previous_and_revoked_versions(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "sealed",
    )
    .await;

    let error = openbao
        .store()
        .revoke(&discord_bot_token(CredentialVersion::new(3).unwrap()))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::BackendUnavailable { backend: StorageBackend::OpenBao, message }
            if message.contains("500") && message.contains("sealed")
    ));
}

struct OpenBaoServer {
    _container: ContainerAsync<GenericImage>,
    address: String,
}

impl OpenBaoServer {
    async fn start() -> Self {
        let container = GenericImage::new(OPENBAO_IMAGE, OPENBAO_IMAGE_TAG)
            .with_wait_for(WaitFor::message_on_stdout(
                "Development mode should NOT be used in production",
            ))
            .with_exposed_port(OPENBAO_PORT.tcp())
            .with_cmd(vec![
                "server",
                "-dev",
                "-dev-root-token-id=dev-only-token",
                "-dev-listen-address=0.0.0.0:8200",
            ])
            .start()
            .await
            .expect("start OpenBao testcontainer");
        let host = container.get_host().await.expect("get OpenBao testcontainer host");
        let port = container
            .get_host_port_ipv4(OPENBAO_PORT)
            .await
            .expect("get OpenBao testcontainer port");
        Self {
            _container: container,
            address: format!("http://{host}:{port}"),
        }
    }

    fn store(&self) -> OpenBaoSecretStore {
        OpenBaoSecretStore::new(&self.address, OPENBAO_DEV_ROOT_TOKEN).unwrap()
    }
}

async fn assert_roundtrip(store: &OpenBaoSecretStore) -> CredentialRef {
    let scope = CredentialScope::integration(
        CredentialOwnerId::new("tenant-e2e").unwrap(),
        SourceKind::GitHub,
        SourceIntegrationId::new("copy-in-out").unwrap(),
    );

    let credential = store
        .put(
            scope,
            CredentialKind::WebhookSecret,
            SecretString::new(TEST_INPUT).unwrap(),
        )
        .await
        .unwrap();
    let material = store.get(&credential).await.unwrap();
    let output = material.as_plaintext().unwrap().as_str();
    let metadata = store.metadata(&credential).await.unwrap();

    assert_eq!(output, TEST_INPUT);
    assert_eq!(metadata.status(), CredentialStatus::Active);
    assert_eq!(metadata.storage_backend(), StorageBackend::OpenBao);
    assert_custom_metadata(store, &credential).await;
    credential
}

async fn assert_custom_metadata(store: &OpenBaoSecretStore, credential: &CredentialRef) {
    let response = store.openbao_metadata(credential).await.unwrap();
    let custom_metadata = response
        .data
        .custom_metadata
        .expect("custom metadata to be populated on write");

    let expected_version = credential.version().get().to_string();
    assert_eq!(
        custom_metadata.get("owner_id").map(String::as_str),
        Some(credential.owner_id().as_str())
    );
    assert_eq!(
        custom_metadata.get("credential_id").map(String::as_str),
        Some(credential.id().as_str())
    );
    assert_eq!(
        custom_metadata.get("credential_kind").map(String::as_str),
        Some(credential.kind().as_str())
    );
    assert_eq!(
        custom_metadata.get("current_version").map(String::as_str),
        Some(expected_version.as_str())
    );

    let created_at = custom_metadata.get("created_at").cloned().unwrap_or_default();
    assert!(!created_at.is_empty());
    assert!(chrono::DateTime::parse_from_rfc3339(&created_at).is_ok());
}

#[tokio::test]
async fn openbao_testcontainer_roundtrips_precise_value() {
    let server = OpenBaoServer::start().await;
    let store = server.store();
    let credential = assert_roundtrip(&store).await;

    assert_eq!(
        store.data_path(&credential),
        "secret/data/trogonai/tenant-e2e/credentials/openbao%3Atenant-e2e%3Agithub%2Fcopy-in-out%3Awebhook_secret"
    );
    assert_eq!(
        store.metadata_path(&credential),
        "secret/metadata/trogonai/tenant-e2e/credentials/openbao%3Atenant-e2e%3Agithub%2Fcopy-in-out%3Awebhook_secret"
    );
}

#[tokio::test]
async fn openbao_testcontainer_destroys_credential_version() {
    let server = OpenBaoServer::start().await;
    let store = server.store();
    let credential = assert_roundtrip(&store).await;

    store
        .destroy(
            &credential,
            &SecretDestroyReason::new("integration test cleanup").unwrap(),
        )
        .await
        .unwrap();

    assert!(matches!(
        store.get(&credential).await,
        Err(SecretStoreError::Unreadable {
            status: CredentialStatus::Destroyed,
            ..
        })
    ));
    assert_eq!(
        store.metadata(&credential).await.unwrap().status(),
        CredentialStatus::Destroyed
    );

    store
        .destroy(
            &credential,
            &SecretDestroyReason::new("integration test cleanup").unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn openbao_testcontainer_rotate_advances_custom_metadata_version() {
    let server = OpenBaoServer::start().await;
    let store = server.store();
    let credential = assert_roundtrip(&store).await;

    let rotated = store
        .rotate(&credential, SecretString::new("rotated-value").unwrap())
        .await
        .unwrap();

    assert_eq!(rotated.version().get(), credential.version().get() + 1);
    assert_custom_metadata(&store, &rotated).await;
}

#[tokio::test]
#[ignore = "requires dev OpenBao from devops/docker/compose"]
async fn openbao_dev_server_roundtrips_precise_value() {
    let address = SystemEnv
        .var("OPENBAO_ADDR")
        .unwrap_or_else(|_| "http://openbao.trogonai.orb.local:8200".to_string());
    let token = SystemEnv
        .var("OPENBAO_TOKEN")
        .unwrap_or_else(|_| "local-dev-root-token".to_string());
    let store = OpenBaoSecretStore::new(address, token).unwrap();
    let credential = assert_roundtrip(&store).await;
    println!(
        "{}",
        json!({
            "credential_ref": credential.to_string(),
            "input": TEST_INPUT,
            "output": TEST_INPUT,
            "data_path": store.data_path(&credential),
            "metadata_path": store.metadata_path(&credential),
        })
    );
}

#[tokio::test]
async fn metadata_treats_a_scheduled_deletion_as_not_yet_revoked() {
    let openbao = FakeOpenBao::start(Some(json!({
        "current_version": 1,
        "versions": {
            "1": { "deletion_time": "2999-01-01T00:00:00Z", "destroyed": false },
        },
    })))
    .await;

    let metadata = openbao
        .store()
        .metadata(&discord_bot_token(CredentialVersion::initial()))
        .await
        .unwrap();

    assert_eq!(metadata.status(), CredentialStatus::Active);
}

#[tokio::test]
async fn metadata_with_an_unparseable_deletion_time_is_a_backend_outage() {
    let openbao = FakeOpenBao::start(Some(json!({
        "current_version": 1,
        "versions": {
            "1": { "deletion_time": "yesterday", "destroyed": false },
        },
    })))
    .await;

    let error = openbao
        .store()
        .metadata(&discord_bot_token(CredentialVersion::initial()))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            ..
        }
    ));
}

#[tokio::test]
async fn redirects_are_not_followed_so_the_token_stays_with_openbao() {
    let elsewhere = FakeOpenBao::start(Some(metadata_with_previous_and_revoked_versions())).await;
    let location = format!("http://{}/v1/secret/metadata/stolen", elsewhere.address);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirector = listener.local_addr().unwrap();
    let app = Router::new().fallback(move || {
        let location = location.clone();
        async move {
            (
                StatusCode::TEMPORARY_REDIRECT,
                [(axum::http::header::LOCATION, location)],
            )
                .into_response()
        }
    });
    tokio::spawn(axum::serve(listener, app).into_future());
    let store = OpenBaoSecretStore::new(format!("http://{redirector}"), OPENBAO_DEV_ROOT_TOKEN).unwrap();

    let error = store
        .metadata(&discord_bot_token(CredentialVersion::initial()))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            ..
        }
    ));
    assert!(elsewhere.requests.lock().await.is_empty());
}
