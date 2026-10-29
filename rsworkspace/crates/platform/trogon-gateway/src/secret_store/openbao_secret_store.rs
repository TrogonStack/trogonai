use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;
use trogon_std::{EmptySecretError, SecretString};
use url::Url;

use super::{
    SecretDestroyReason, SecretMaterial, SecretStoreDestroy, SecretStoreError, SecretStoreGet, SecretStoreMetadata,
    SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
};
use crate::credential::commands::domain::{
    CredentialFingerprint, CredentialId, CredentialIdError, CredentialKind, CredentialMetadata, CredentialOwnerId,
    CredentialOwnerIdError, CredentialPath, CredentialRef, CredentialScope, CredentialStatus, CredentialVersion,
    SourceKind, StorageBackend,
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
        CredentialPath::from(credential)
            .segments()
            .map(encode_path_segment)
            .join("/")
    }

    fn endpoint(&self, kind: OpenBaoEndpoint, credential: &CredentialRef) -> Result<Url, SecretStoreError> {
        self.address
            .join(&format!(
                "/v1/{}/{}/{}",
                self.mount,
                kind.as_str(),
                self.credential_path(credential)
            ))
            .map_err(|error| SecretStoreError::BackendUnavailable {
                backend: StorageBackend::OpenBao,
                message: format!("failed to build endpoint url: {error}"),
            })
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
    EmptyToken(#[source] EmptySecretError),
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
                self.authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Data, &initial_ref)?))
                    .json(&OpenBaoWriteRequest::new(&value)),
            )
            .await?;
        let credential = self.credential_ref(&scope, kind, response.data.version()?)?;
        self.write_custom_metadata(&credential).await?;
        Ok(credential)
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

        let mut url = self.endpoint(OpenBaoEndpoint::Data, credential)?;
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
        if !status.is_writable() {
            return Err(SecretStoreError::Unwritable {
                credential: credential.clone(),
                status,
            });
        }

        let response: OpenBaoWriteResponse = self
            .send_json(
                self.authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Data, credential)?))
                    .json(&OpenBaoWriteRequest::new(&value)),
            )
            .await?;

        let rotated = credential.with_version(response.data.version()?);
        self.write_custom_metadata(&rotated).await?;
        Ok(rotated)
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
            self.authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Delete, credential)?))
                .json(&OpenBaoVersionsRequest { versions: &versions }),
        )
        .await
    }
}

impl SecretStoreDestroy for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn destroy(&self, credential: &CredentialRef, reason: &SecretDestroyReason) -> Result<(), Self::Error> {
        info!(credential = %credential, reason = %reason, "destroying credential material in OpenBao");

        self.send_empty(
            self.authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Destroy, credential)?))
                .json(&OpenBaoVersionsRequest {
                    versions: &[credential.version().get()],
                }),
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

        let status = if version.destroyed {
            CredentialStatus::Destroyed
        } else if !version.deletion_time.is_empty() {
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
            .send_json(self.authorize(self.http.get(self.endpoint(OpenBaoEndpoint::Metadata, credential)?)))
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

    async fn write_custom_metadata(&self, credential: &CredentialRef) -> Result<(), SecretStoreError> {
        self.send_empty(
            self.authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Metadata, credential)?))
                .json(&OpenBaoCustomMetadataRequest::new(credential, Utc::now())),
        )
        .await
    }
}

#[derive(Clone, Copy)]
enum OpenBaoEndpoint {
    Data,
    Delete,
    Destroy,
    Metadata,
}

impl OpenBaoEndpoint {
    fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Delete => "delete",
            Self::Destroy => "destroy",
            Self::Metadata => "metadata",
        }
    }
}

#[derive(Serialize)]
struct OpenBaoWriteRequest<'a> {
    data: OpenBaoWriteRequestData<'a>,
}

impl<'a> OpenBaoWriteRequest<'a> {
    fn new(value: &'a SecretString) -> Self {
        Self {
            data: OpenBaoWriteRequestData { value: value.as_str() },
        }
    }
}

#[derive(Serialize)]
struct OpenBaoWriteRequestData<'a> {
    value: &'a str,
}

#[derive(Serialize)]
struct OpenBaoVersionsRequest<'a> {
    versions: &'a [u64],
}

#[derive(Serialize)]
struct OpenBaoCustomMetadataRequest<'a> {
    custom_metadata: OpenBaoCustomMetadata<'a>,
}

impl<'a> OpenBaoCustomMetadataRequest<'a> {
    fn new(credential: &'a CredentialRef, created_at: DateTime<Utc>) -> Self {
        Self {
            custom_metadata: OpenBaoCustomMetadata {
                created_at: created_at.to_rfc3339(),
                credential_id: credential.id().as_str(),
                credential_kind: credential.kind().as_str(),
                current_version: credential.version().get().to_string(),
                owner_id: credential.owner_id().as_str(),
            },
        }
    }
}

#[derive(Serialize)]
struct OpenBaoCustomMetadata<'a> {
    created_at: String,
    credential_id: &'a str,
    credential_kind: &'a str,
    current_version: String,
    owner_id: &'a str,
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
    #[serde(default)]
    #[allow(dead_code, reason = "populated for reconciliation tooling; read from tests today")]
    custom_metadata: Option<BTreeMap<String, String>>,
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
mod tests;
