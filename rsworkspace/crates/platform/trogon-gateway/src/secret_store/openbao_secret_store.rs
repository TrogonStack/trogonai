use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use trogon_std::{EmptySecret, SecretString};
use url::Url;

use super::{
    SecretMaterial, SecretStoreError, SecretStoreGet, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke,
    SecretStoreRotate,
};
use crate::credential::commands::domain::{
    CredentialFingerprint, CredentialId, CredentialIdError, CredentialKind, CredentialMetadata, CredentialOwnerId,
    CredentialOwnerIdError, CredentialRef, CredentialScope, CredentialStatus, CredentialVersion, SourceKind,
    StorageBackend,
};
use crate::source_integration_id::SourceIntegrationId;

pub(crate) fn openbao_credential_id(
    scope: &CredentialScope,
    kind: CredentialKind,
) -> Result<CredentialId, CredentialIdError> {
    CredentialId::new(format!(
        "openbao:{}:{}:{}",
        scope.owner_id(),
        scope.scope_key(),
        kind.as_str()
    ))
}

pub(crate) fn openbao_credential_ref_from_id(
    id: CredentialId,
    version: CredentialVersion,
) -> Result<CredentialRef, OpenBaoCredentialIdParseError> {
    let value = id.as_str();
    let Some(rest) = value.strip_prefix("openbao:") else {
        return Err(OpenBaoCredentialIdParseError::MissingPrefix { id });
    };
    let Some((owner_id, scope_key, kind)) = split_openbao_credential_id(rest) else {
        return Err(OpenBaoCredentialIdParseError::InvalidShape { id });
    };
    let owner_id = CredentialOwnerId::new(owner_id)
        .map_err(|source| OpenBaoCredentialIdParseError::InvalidOwner { id: id.clone(), source })?;
    let kind =
        CredentialKind::parse(kind).ok_or_else(|| OpenBaoCredentialIdParseError::InvalidKind { id: id.clone() })?;
    let scope = openbao_scope_from_scope_key(owner_id, scope_key)
        .map_err(|source| OpenBaoCredentialIdParseError::InvalidScope { id: id.clone(), source })?;

    Ok(CredentialRef::new(id, version, &scope, kind))
}

fn split_openbao_credential_id(value: &str) -> Option<(&str, &str, &str)> {
    let (owner_and_scope, kind) = value.rsplit_once(':')?;
    let (owner_id, scope_key) = owner_and_scope.rsplit_once(':')?;
    Some((owner_id, scope_key, kind))
}

fn openbao_scope_from_scope_key(
    owner_id: CredentialOwnerId,
    scope_key: &str,
) -> Result<CredentialScope, OpenBaoScopeKeyParseError> {
    if let Some(source) = SourceKind::parse(scope_key) {
        return Ok(CredentialScope::source(owner_id, source));
    }

    let Some((source, integration_id)) = scope_key.split_once('/') else {
        return Err(OpenBaoScopeKeyParseError::MissingSource);
    };
    let source = SourceKind::parse(source).ok_or(OpenBaoScopeKeyParseError::UnknownSource)?;
    let integration_id =
        SourceIntegrationId::new(integration_id).map_err(OpenBaoScopeKeyParseError::InvalidIntegration)?;
    Ok(CredentialScope::integration(owner_id, source, integration_id))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenBaoCredentialIdParseError {
    #[error("OpenBao credential id is missing expected prefix: {id}")]
    MissingPrefix { id: CredentialId },
    #[error("OpenBao credential id has invalid shape: {id}")]
    InvalidShape { id: CredentialId },
    #[error("OpenBao credential id has invalid owner: {id}")]
    InvalidOwner {
        id: CredentialId,
        #[source]
        source: CredentialOwnerIdError,
    },
    #[error("OpenBao credential id has invalid scope: {id}")]
    InvalidScope {
        id: CredentialId,
        #[source]
        source: OpenBaoScopeKeyParseError,
    },
    #[error("OpenBao credential id has invalid kind: {id}")]
    InvalidKind { id: CredentialId },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenBaoScopeKeyParseError {
    #[error("scope key is missing source")]
    MissingSource,
    #[error("scope key source is unknown")]
    UnknownSource,
    #[error("scope key integration id is invalid")]
    InvalidIntegration(#[source] crate::source_integration_id::SourceIntegrationIdError),
}

#[derive(Clone)]
pub struct OpenBaoSecretStore {
    address: Url,
    token: SecretString,
    mount: OpenBaoMount,
    http: reqwest::Client,
}

impl OpenBaoSecretStore {
    pub fn new(address: impl AsRef<str>, token: impl AsRef<str>) -> Result<Self, OpenBaoSecretStoreConfigError> {
        Self::with_mount(address, token, OpenBaoMount::default())
    }

    pub fn with_mount(
        address: impl AsRef<str>,
        token: impl AsRef<str>,
        mount: OpenBaoMount,
    ) -> Result<Self, OpenBaoSecretStoreConfigError> {
        let address = Url::parse(address.as_ref().trim_end_matches('/'))
            .map_err(OpenBaoSecretStoreConfigError::InvalidAddress)?;
        let token = SecretString::new(token).map_err(OpenBaoSecretStoreConfigError::EmptyToken)?;
        Ok(Self {
            address,
            token,
            mount,
            http: reqwest::Client::new(),
        })
    }

    pub fn data_path(&self, credential: &CredentialRef) -> String {
        format!("{}/data/{}", self.mount, self.credential_path(credential))
    }

    pub fn metadata_path(&self, credential: &CredentialRef) -> String {
        format!("{}/metadata/{}", self.mount, self.credential_path(credential))
    }

    fn credential_ref(
        &self,
        scope: &CredentialScope,
        kind: CredentialKind,
        version: CredentialVersion,
    ) -> Result<CredentialRef, SecretStoreError> {
        let id = openbao_credential_id(scope, kind).map_err(|error| self.backend_error(error.to_string()))?;
        Ok(CredentialRef::new(id, version, scope, kind))
    }

    fn credential_path(&self, credential: &CredentialRef) -> String {
        format!(
            "trogonai/{}/credentials/{}",
            encode_path_segment(credential.owner_id().as_str()),
            encode_path_segment(credential.id().as_str())
        )
    }

    fn endpoint(&self, kind: OpenBaoEndpoint, credential: &CredentialRef) -> Url {
        self.address
            .join(&format!(
                "/v1/{}/{}/{}",
                self.mount,
                kind.as_str(),
                self.credential_path(credential)
            ))
            .expect("validated OpenBao URL must accept API paths")
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header("X-Vault-Token", self.token.as_str())
    }

    async fn send_json<T: for<'de> Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, SecretStoreError> {
        let response = request
            .send()
            .await
            .map_err(|error| self.backend_error(error.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| self.backend_error(error.to_string()))?;

        if !status.is_success() {
            return Err(self.status_error(status, body));
        }

        serde_json::from_str(&body).map_err(|error| self.backend_error(error.to_string()))
    }

    async fn send_empty(&self, request: reqwest::RequestBuilder) -> Result<(), SecretStoreError> {
        let response = request
            .send()
            .await
            .map_err(|error| self.backend_error(error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response
            .text()
            .await
            .map_err(|error| self.backend_error(error.to_string()))?;
        Err(self.status_error(status, body))
    }

    fn status_error(&self, status: StatusCode, body: String) -> SecretStoreError {
        self.backend_error(format!("OpenBao returned {status}: {body}"))
    }

    fn backend_error(&self, message: String) -> SecretStoreError {
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            message,
        }
    }
}

impl fmt::Debug for OpenBaoSecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBaoSecretStore")
            .field("address", &self.address)
            .field("mount", &self.mount)
            .field("token", &"***")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenBaoMount(Arc<str>);

impl OpenBaoMount {
    pub fn new(value: impl AsRef<str>) -> Result<Self, OpenBaoMountError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(OpenBaoMountError::Empty);
        }
        for ch in value.chars() {
            if !is_mount_char(ch) {
                return Err(OpenBaoMountError::InvalidCharacter(ch));
            }
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for OpenBaoMount {
    fn default() -> Self {
        Self(Arc::from("secret"))
    }
}

impl fmt::Display for OpenBaoMount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OpenBaoMountError {
    #[error("OpenBao mount must not be empty")]
    Empty,
    #[error("OpenBao mount contains invalid character '{0}'")]
    InvalidCharacter(char),
}

fn is_mount_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

#[derive(Debug, thiserror::Error)]
pub enum OpenBaoSecretStoreConfigError {
    #[error("invalid OpenBao address: {0}")]
    InvalidAddress(#[source] url::ParseError),
    #[error("invalid OpenBao token: {0}")]
    EmptyToken(#[source] EmptySecret),
}

impl SecretStorePut for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn put(
        &self,
        scope: CredentialScope,
        kind: CredentialKind,
        value: SecretString,
    ) -> Result<CredentialRef, Self::Error> {
        let initial_ref = self.credential_ref(&scope, kind, CredentialVersion::initial())?;
        let response: OpenBaoWriteResponse = self
            .send_json(
                self.authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Data, &initial_ref)))
                    .json(&json!({
                        "data": {
                            "value": value.as_str(),
                        },
                    })),
            )
            .await?;
        self.credential_ref(&scope, kind, response.data.version()?)
    }
}

impl SecretStoreGet for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn get(&self, credential: &CredentialRef) -> Result<SecretMaterial, Self::Error> {
        let metadata = self.metadata(credential).await?;
        let status = metadata.status();
        if !status.is_readable() {
            return Err(SecretStoreError::Unreadable {
                credential: credential.clone(),
                status,
            });
        }

        let mut url = self.endpoint(OpenBaoEndpoint::Data, credential);
        url.query_pairs_mut()
            .append_pair("version", &credential.version().get().to_string());
        let response: OpenBaoReadResponse = self.send_json(self.authorize(self.http.get(url))).await?;
        let value =
            SecretString::new(response.data.data.value).map_err(|error| self.backend_error(error.to_string()))?;
        Ok(SecretMaterial::plaintext(value))
    }
}

impl SecretStoreRotate for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn rotate(&self, credential: &CredentialRef, value: SecretString) -> Result<CredentialRef, Self::Error> {
        let metadata = self.metadata(credential).await?;
        let status = metadata.status();
        if status != CredentialStatus::Active {
            return Err(SecretStoreError::Unwritable {
                credential: credential.clone(),
                status,
            });
        }

        let response: OpenBaoWriteResponse = self
            .send_json(
                self.authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Data, credential)))
                    .json(&json!({
                        "data": {
                            "value": value.as_str(),
                        },
                    })),
            )
            .await?;

        Ok(credential.with_version(response.data.version()?))
    }
}

impl SecretStoreRevoke for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn revoke(&self, credential: &CredentialRef) -> Result<(), Self::Error> {
        let metadata = self.openbao_metadata(credential).await?;
        let versions: Vec<u64> = (1..=metadata.data.current_version).collect();
        if versions.is_empty() {
            return Err(SecretStoreError::Missing {
                credential: credential.clone(),
            });
        }

        self.send_empty(
            self.authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Delete, credential)))
                .json(&json!({
                    "versions": versions,
                })),
        )
        .await
    }
}

impl SecretStoreMetadata for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        let metadata = self.openbao_metadata(credential).await?;
        let version = metadata
            .data
            .versions
            .get(&credential.version().get().to_string())
            .ok_or_else(|| SecretStoreError::Missing {
                credential: credential.clone(),
            })?;

        let status = if version.destroyed || !version.deletion_time.is_empty() {
            CredentialStatus::Revoked
        } else if credential.version().get() == metadata.data.current_version {
            CredentialStatus::Active
        } else {
            CredentialStatus::Previous
        };

        let fingerprint = CredentialFingerprint::new(openbao_fingerprint(
            &self.metadata_path(credential),
            credential.version().get(),
        ))
        .map_err(|error| self.backend_error(error.to_string()))?;

        Ok(CredentialMetadata::new(
            credential.clone(),
            status,
            StorageBackend::OpenBao,
            fingerprint,
        ))
    }
}

fn openbao_fingerprint(metadata_path: &str, version: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(metadata_path.as_bytes());
    hasher.update(b"#");
    hasher.update(version.to_string().as_bytes());
    format!("openbao:{}", hex::encode(hasher.finalize()))
}

impl OpenBaoSecretStore {
    async fn openbao_metadata(&self, credential: &CredentialRef) -> Result<OpenBaoMetadataResponse, SecretStoreError> {
        match self
            .send_json(self.authorize(self.http.get(self.endpoint(OpenBaoEndpoint::Metadata, credential))))
            .await
        {
            Ok(metadata) => Ok(metadata),
            Err(SecretStoreError::BackendUnavailable { message, .. }) if message.contains("404") => {
                Err(SecretStoreError::Missing {
                    credential: credential.clone(),
                })
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Copy)]
enum OpenBaoEndpoint {
    Data,
    Delete,
    Metadata,
}

impl OpenBaoEndpoint {
    fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Delete => "delete",
            Self::Metadata => "metadata",
        }
    }
}

#[derive(Deserialize)]
struct OpenBaoWriteResponse {
    data: OpenBaoWriteData,
}

#[derive(Deserialize)]
struct OpenBaoWriteData {
    version: u64,
}

impl OpenBaoWriteData {
    fn version(&self) -> Result<CredentialVersion, SecretStoreError> {
        CredentialVersion::new(self.version).map_err(|error| SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            message: error.to_string(),
        })
    }
}

#[derive(Deserialize)]
struct OpenBaoReadResponse {
    data: OpenBaoReadData,
}

#[derive(Deserialize)]
struct OpenBaoReadData {
    data: OpenBaoSecretValue,
}

#[derive(Deserialize)]
struct OpenBaoSecretValue {
    value: String,
}

#[derive(Deserialize)]
struct OpenBaoMetadataResponse {
    data: OpenBaoMetadata,
}

#[derive(Deserialize)]
struct OpenBaoMetadata {
    current_version: u64,
    versions: BTreeMap<String, OpenBaoVersionMetadata>,
}

#[derive(Deserialize)]
struct OpenBaoVersionMetadata {
    #[serde(default)]
    deletion_time: String,
    #[serde(default)]
    destroyed: bool,
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::env;

    use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};
    use crate::source_integration_id::SourceIntegrationId;
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
        credential
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
    #[ignore = "requires dev OpenBao from devops/docker/compose"]
    async fn openbao_dev_server_roundtrips_precise_value() {
        let address = env::var("OPENBAO_ADDR").unwrap_or_else(|_| "http://openbao.trogonai.orb.local:8200".to_string());
        let token = env::var("OPENBAO_TOKEN").unwrap_or_else(|_| "dev-only-token".to_string());
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
}
