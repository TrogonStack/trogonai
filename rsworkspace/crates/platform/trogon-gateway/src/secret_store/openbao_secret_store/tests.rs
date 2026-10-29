use std::env;

use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};
use crate::source_integration_id::SourceIntegrationId;
use serde_json::json;
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

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
fn openbao_fingerprint_stays_within_value_object_limit() {
    let fingerprint = openbao_fingerprint(
        "secret/metadata/trogonai/tenant-compose-smoke/credentials/openbao%3Atenant-compose-smoke%3Agithub%2Fcompose-smoke%3Awebhook_secret",
        2,
    );

    assert!(CredentialFingerprint::new(&fingerprint).is_ok());
    assert_eq!(fingerprint.len(), "openbao:".len() + 64);
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
    let address = env::var("OPENBAO_ADDR").unwrap_or_else(|_| "http://openbao.trogonai.orb.local:8200".to_string());
    let token = env::var("OPENBAO_TOKEN").unwrap_or_else(|_| "local-dev-root-token".to_string());
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
