use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use trogon_std::{EmptySecret, SecretString};
use url::Url;

use super::{
    CredentialFingerprint, CredentialId, CredentialKind, CredentialMetadata, CredentialRef, CredentialScope,
    CredentialStatus, CredentialVersion, SecretMaterial, SecretStoreError, SecretStoreGet, SecretStoreMetadata,
    SecretStorePut, SecretStoreRevoke, SecretStoreRotate, StorageBackend,
};

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
        let id = CredentialId::new(format!("openbao:{}:{}", scope.scope_key(), kind.as_str()))
            .map_err(|error| self.backend_error(error.to_string()))?;
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

        Ok(CredentialRef::new(
            credential.id().clone(),
            response.data.version()?,
            &CredentialScope::source(credential.owner_id().clone(), credential.source()),
            credential.kind(),
        ))
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

        let fingerprint = CredentialFingerprint::new(format!(
            "openbao:{}#{}",
            self.metadata_path(credential),
            credential.version().get()
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

    use crate::secret_store::{CredentialOwnerId, SourceKind};
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
            encode_path_segment("openbao:github/primary:webhook_secret"),
            "openbao%3Agithub%2Fprimary%3Awebhook_secret"
        );
    }

    #[test]
    fn default_mount_is_secret() {
        assert_eq!(OpenBaoMount::default().as_str(), "secret");
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
            "secret/data/trogonai/tenant-e2e/credentials/openbao%3Agithub%2Fcopy-in-out%3Awebhook_secret"
        );
        assert_eq!(
            store.metadata_path(&credential),
            "secret/metadata/trogonai/tenant-e2e/credentials/openbao%3Agithub%2Fcopy-in-out%3Awebhook_secret"
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
