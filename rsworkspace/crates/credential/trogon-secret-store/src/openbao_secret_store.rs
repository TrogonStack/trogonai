use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use trogon_credential_domain::{
    CredentialIdentity, CredentialKey, CredentialMetadata, CredentialPath, CredentialRef, CredentialStatus,
    CredentialVersion,
};
use trogon_std::SecretString;
use url::Url;

use super::constants::{RELEASE_REQUEST_TIMEOUT, STATIC_TOKEN_REQUEST_TIMEOUT};
use super::openbao_app_role_session::OpenBaoClock;
use super::openbao_http::{send_empty, send_json};
use super::{
    OpenBaoAppRole, OpenBaoAppRoleSession, OpenBaoCallError, OpenBaoCaller, OpenBaoLoginError,
    OpenBaoSecretStoreConfigError, OpenBaoTokenSource, SecretDestroyReason, SecretMaterial, SecretStoreDestroy,
    SecretStoreError, SecretStoreGet, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
    fingerprint,
};

#[derive(Clone)]
pub struct OpenBaoSecretStore {
    address: Url,
    tokens: OpenBaoTokenSource,
    mount: OpenBaoMount,
    http: reqwest::Client,
}

impl OpenBaoSecretStore {
    pub fn new(
        address: impl AsRef<str>,
        token: impl AsRef<str>,
        transport: OpenBaoTransport,
    ) -> Result<Self, OpenBaoSecretStoreConfigError> {
        Self::with_mount(address, token, transport, OpenBaoMount::default())
    }

    pub fn with_mount(
        address: impl AsRef<str>,
        token: impl AsRef<str>,
        transport: OpenBaoTransport,
        mount: OpenBaoMount,
    ) -> Result<Self, OpenBaoSecretStoreConfigError> {
        let address = parse_address(address, transport)?;
        let token = SecretString::new(token).map_err(OpenBaoSecretStoreConfigError::EmptyToken)?;
        Ok(Self {
            address,
            tokens: OpenBaoTokenSource::Static(token),
            mount,
            http: http_client(STATIC_TOKEN_REQUEST_TIMEOUT)?,
        })
    }

    /// Logs in with `approle` before returning, so a store that cannot
    /// authenticate is never built.
    pub(crate) async fn login(
        address: impl AsRef<str>,
        transport: OpenBaoTransport,
        mount: OpenBaoMount,
        approle: OpenBaoAppRole,
        clock: OpenBaoClock,
    ) -> Result<Self, OpenBaoLoginError> {
        let address = parse_address(address, transport)?;
        let http = http_client(RELEASE_REQUEST_TIMEOUT)?;
        let session = OpenBaoAppRoleSession::start(approle, clock, &http, &address)
            .await
            .map_err(OpenBaoLoginError::refused)?;
        Ok(Self {
            address,
            tokens: OpenBaoTokenSource::AppRole(Arc::new(session)),
            mount,
            http,
        })
    }

    pub fn data_path(&self, key: &CredentialKey) -> String {
        format!("{}/data/{}", self.mount, credential_path(key))
    }

    pub fn metadata_path(&self, key: &CredentialKey) -> String {
        format!("{}/metadata/{}", self.mount, credential_path(key))
    }

    pub(crate) fn address(&self) -> &Url {
        &self.address
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The token this store holds, renewed or replaced first when it is close
    /// to expiry.
    pub(crate) async fn token(&self) -> Result<SecretString, SecretStoreError> {
        self.tokens.token(&self.http, &self.address).await
    }

    async fn caller(&self) -> Result<OpenBaoCaller, SecretStoreError> {
        Ok(OpenBaoCaller::new(self.token().await?))
    }

    fn endpoint(&self, kind: OpenBaoEndpoint, key: &CredentialKey) -> Result<Url, SecretStoreError> {
        self.address
            .join(&format!(
                "/v1/{}/{}/{}",
                self.mount,
                kind.as_str(),
                credential_path(key)
            ))
            .map_err(|error| OpenBaoCallError::InvalidEndpoint(error).into_store_error())
    }

    fn backend_error(&self, message: String) -> SecretStoreError {
        SecretStoreError::BackendUnavailable { message }
    }
}

fn parse_address(address: impl AsRef<str>, transport: OpenBaoTransport) -> Result<Url, OpenBaoSecretStoreConfigError> {
    let address =
        Url::parse(address.as_ref().trim_end_matches('/')).map_err(OpenBaoSecretStoreConfigError::InvalidAddress)?;
    transport.admit(&address)?;
    Ok(address)
}

fn http_client(timeout: Duration) -> Result<reqwest::Client, OpenBaoSecretStoreConfigError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(OpenBaoSecretStoreConfigError::HttpClient)
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
        if matches!(value, "." | "..") {
            return Err(OpenBaoMountError::DotSegment);
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
    #[error("OpenBao mount must not be a dot path segment")]
    DotSegment,
}

fn is_mount_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenBaoTransport {
    TlsRequired,
    PlaintextAllowed,
}

impl OpenBaoTransport {
    fn admit(self, address: &Url) -> Result<(), OpenBaoSecretStoreConfigError> {
        match (self, address.scheme()) {
            (Self::TlsRequired, "http") => Err(OpenBaoSecretStoreConfigError::PlaintextAddress),
            _ => Ok(()),
        }
    }
}

impl SecretStorePut for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn put(&self, identity: CredentialIdentity, value: SecretString) -> Result<CredentialRef, Self::Error> {
        let caller = self.caller().await?;
        let response: OpenBaoWriteResponse = send_json(
            caller
                .authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Data, &identity.key())?))
                .json(&OpenBaoWriteRequest::new(&value)),
        )
        .await
        .map_err(OpenBaoCallError::into_store_error)?;
        let credential = CredentialRef::new(identity, response.data.version()?);
        self.describe_stored(&credential, &caller).await;
        Ok(credential)
    }
}

impl SecretStoreGet for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn get(&self, credential: &CredentialRef) -> Result<SecretMaterial, Self::Error> {
        self.read_material(credential, &self.caller().await?).await
    }
}

impl SecretStoreRotate for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn rotate(&self, credential: &CredentialRef, value: SecretString) -> Result<CredentialRef, Self::Error> {
        let caller = self.caller().await?;
        let metadata = self.metadata_as(credential, &caller).await?;
        let status = metadata.status();
        if !status.is_writable() {
            return Err(SecretStoreError::Unwritable {
                credential: credential.clone(),
                status,
            });
        }

        let response: OpenBaoWriteResponse = send_json(
            caller
                .authorize(self.http.post(self.endpoint(OpenBaoEndpoint::Data, &credential.key())?))
                .json(&OpenBaoWriteRequest::new(&value)),
        )
        .await
        .map_err(OpenBaoCallError::into_store_error)?;

        let rotated = credential.with_version(response.data.version()?);
        self.describe_stored(&rotated, &caller).await;
        Ok(rotated)
    }
}

impl SecretStoreRevoke for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn revoke(&self, credential: &CredentialRef) -> Result<(), Self::Error> {
        let caller = self.caller().await?;
        let metadata = self.openbao_metadata_as(credential, &caller).await?;
        match metadata.data.status(credential.version(), Utc::now()) {
            None => {
                return Err(SecretStoreError::Missing {
                    credential: credential.clone(),
                });
            }
            Some(CredentialStatus::Destroyed) => {
                return Err(SecretStoreError::Unwritable {
                    credential: credential.clone(),
                    status: CredentialStatus::Destroyed,
                });
            }
            Some(_) => {}
        }

        let versions: Vec<u64> = (1..=metadata.data.current_version).collect();

        send_empty(
            caller
                .authorize(
                    self.http
                        .post(self.endpoint(OpenBaoEndpoint::Delete, &credential.key())?),
                )
                .json(&OpenBaoVersionsRequest { versions: &versions }),
        )
        .await
        .map_err(OpenBaoCallError::into_store_error)
    }
}

impl SecretStoreDestroy for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn destroy(&self, credential: &CredentialRef, reason: &SecretDestroyReason) -> Result<(), Self::Error> {
        let caller = self.caller().await?;
        let metadata = self.openbao_metadata_as(credential, &caller).await?;
        if metadata.data.status(credential.version(), Utc::now()).is_none() {
            return Err(SecretStoreError::Missing {
                credential: credential.clone(),
            });
        }
        info!(credential = %credential, reason = %reason, "destroying credential material in OpenBao");

        let versions: Vec<u64> = (1..=metadata.data.current_version).collect();

        send_empty(
            caller
                .authorize(
                    self.http
                        .post(self.endpoint(OpenBaoEndpoint::Destroy, &credential.key())?),
                )
                .json(&OpenBaoVersionsRequest { versions: &versions }),
        )
        .await
        .map_err(OpenBaoCallError::into_store_error)
    }
}

impl SecretStoreMetadata for OpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        self.metadata_as(credential, &self.caller().await?).await
    }
}

impl OpenBaoSecretStore {
    /// Reads a credential's material as `caller`, refusing a version that is
    /// no longer readable before the material is ever fetched.
    pub(crate) async fn read_material(
        &self,
        credential: &CredentialRef,
        caller: &OpenBaoCaller,
    ) -> Result<SecretMaterial, SecretStoreError> {
        let metadata = self.metadata_as(credential, caller).await?;
        let status = metadata.status();
        if !status.is_readable() {
            return Err(SecretStoreError::Unreadable {
                credential: credential.clone(),
                status,
            });
        }

        let mut url = self.endpoint(OpenBaoEndpoint::Data, &credential.key())?;
        url.query_pairs_mut()
            .append_pair("version", &credential.version().get().to_string());
        let response: OpenBaoReadResponse = send_json(caller.authorize(self.http.get(url)))
            .await
            .map_err(OpenBaoCallError::into_store_error)?;
        let value =
            SecretString::new(response.data.data.value).map_err(|error| self.backend_error(error.to_string()))?;
        Ok(SecretMaterial::plaintext(value))
    }

    async fn metadata_as(
        &self,
        credential: &CredentialRef,
        caller: &OpenBaoCaller,
    ) -> Result<CredentialMetadata, SecretStoreError> {
        let metadata = self.openbao_metadata_as(credential, caller).await?;
        let status =
            metadata
                .data
                .status(credential.version(), Utc::now())
                .ok_or_else(|| SecretStoreError::Missing {
                    credential: credential.clone(),
                })?;

        let fingerprint = fingerprint::digest(
            "openbao",
            format!(
                "{}#{}",
                self.metadata_path(&credential.key()),
                credential.version().get()
            ),
        )
        .map_err(|error| self.backend_error(error.to_string()))?;

        Ok(CredentialMetadata::new(credential.clone(), status, fingerprint))
    }

    #[cfg(test)]
    async fn openbao_metadata(&self, credential: &CredentialRef) -> Result<OpenBaoMetadataResponse, SecretStoreError> {
        self.openbao_metadata_as(credential, &self.caller().await?).await
    }

    async fn openbao_metadata_as(
        &self,
        credential: &CredentialRef,
        caller: &OpenBaoCaller,
    ) -> Result<OpenBaoMetadataResponse, SecretStoreError> {
        match send_json(
            caller.authorize(
                self.http
                    .get(self.endpoint(OpenBaoEndpoint::Metadata, &credential.key())?),
            ),
        )
        .await
        {
            Ok(metadata) => Ok(metadata),
            Err(OpenBaoCallError::Status {
                status: StatusCode::NOT_FOUND,
                ..
            }) => Err(SecretStoreError::Missing {
                credential: credential.clone(),
            }),
            Err(error) => Err(error.into_store_error()),
        }
    }

    async fn describe_stored(&self, credential: &CredentialRef, caller: &OpenBaoCaller) {
        if let Err(error) = self.write_custom_metadata(credential, caller).await {
            warn!(credential = %credential, error = %error, "stored credential version without its OpenBao custom metadata");
        }
    }

    async fn write_custom_metadata(
        &self,
        credential: &CredentialRef,
        caller: &OpenBaoCaller,
    ) -> Result<(), SecretStoreError> {
        send_empty(
            caller
                .authorize(
                    self.http
                        .post(self.endpoint(OpenBaoEndpoint::Metadata, &credential.key())?),
                )
                .json(&OpenBaoCustomMetadataRequest::new(credential, Utc::now())),
        )
        .await
        .map_err(OpenBaoCallError::into_store_error)
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
                credential_id: credential.id().to_string(),
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
    credential_id: String,
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

impl OpenBaoMetadata {
    fn status(&self, version: CredentialVersion, now: DateTime<Utc>) -> Option<CredentialStatus> {
        let metadata = self.versions.get(&version.get().to_string())?;
        let status = if metadata.destroyed {
            CredentialStatus::Destroyed
        } else if metadata.deletion_time.is_some_and(|deletion_time| deletion_time <= now) {
            CredentialStatus::Revoked
        } else if version.get() == self.current_version {
            CredentialStatus::Active
        } else {
            CredentialStatus::Previous
        };
        Some(status)
    }
}

#[derive(Deserialize)]
struct OpenBaoVersionMetadata {
    #[serde(default, deserialize_with = "deserialize_deletion_time")]
    deletion_time: Option<DateTime<Utc>>,
    #[serde(default)]
    destroyed: bool,
}

fn deserialize_deletion_time<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    if raw.is_empty() {
        return Ok(None);
    }
    DateTime::parse_from_rfc3339(&raw)
        .map(|deletion_time| Some(deletion_time.with_timezone(&Utc)))
        .map_err(serde::de::Error::custom)
}

fn credential_path(key: &CredentialKey) -> String {
    CredentialPath::new(key.clone())
        .segments()
        .map(|segment| encode_path_segment(&segment))
        .join("/")
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
