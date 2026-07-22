use std::io::Cursor;

use buffa::{EnumValue, MessageField};
use bytes::Bytes;
use time::OffsetDateTime;
use trogon_nats::jetstream::{ObjectStoreDelete, ObjectStoreGet, ObjectStorePut};
use trogonai_session_contracts::{
    ArtifactId, ArtifactMetadata, ArtifactSourceAvailability, ArtifactRef, ArtifactRetentionPolicy, EncryptionStatus,
    EventId, SCHEMA_VERSION_V1, SessionId, TextToolResult, ToolCallResult, ToolExecutionId,
};

use crate::checksum::sha256_hex;
use crate::config::ArtifactStoreConfig;
use crate::error::ArtifactStoreError;
use crate::nats::{artifact_object_key, artifact_storage_ref, object_key_from_storage_ref};
use crate::preview::build_preview;
use crate::telemetry;

/// How artifact bytes are persisted for a store operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactStorageMode {
    Inline,
    ClaimCheck,
}

/// Request to persist session artifact content.
#[derive(Clone, Debug, PartialEq)]
pub struct StoreArtifactRequest {
    pub session_id: SessionId,
    pub event_id: EventId,
    pub tool_execution_id: Option<ToolExecutionId>,
    pub mime: String,
    pub content: Bytes,
    pub retention_policy: ArtifactRetentionPolicy,
    pub permission_scope: String,
    pub encryption_status: EncryptionStatus,
    // § Politica para imagenes URL y Base64: provenance kept as metadata so a URL/Base64
    // source is never the canonical truth (defaults are non-image / STORED).
    pub availability: ArtifactSourceAvailability,
    pub source_url: Option<String>,
    pub fetched_at: Option<buffa_types::google::protobuf::Timestamp>,
    pub source_encoding: Option<String>,
    pub declared_mime: Option<String>,
    pub decoded_mime: Option<String>,
}

impl StoreArtifactRequest {
    pub fn new(
        session_id: SessionId,
        event_id: EventId,
        mime: impl Into<String>,
        content: Bytes,
    ) -> Self {
        Self {
            session_id,
            event_id,
            tool_execution_id: None,
            mime: mime.into(),
            content,
            retention_policy: ArtifactRetentionPolicy::Session,
            permission_scope: crate::config::DEFAULT_PERMISSION_SCOPE.to_string(),
            encryption_status: EncryptionStatus::None,
            availability: ArtifactSourceAvailability::Stored,
            source_url: None,
            fetched_at: None,
            source_encoding: None,
            declared_mime: None,
            decoded_mime: None,
        }
    }

    pub fn with_tool_execution_id(mut self, tool_execution_id: ToolExecutionId) -> Self {
        self.tool_execution_id = Some(tool_execution_id);
        self
    }

    /// Build a request from a Base64 image source (§ Imagen Base64): decode to raw bytes
    /// (never store the Base64 string), validate the real MIME against the declared one,
    /// and keep `source_encoding=base64`, `declared_mime`, `decoded_mime` as metadata.
    /// `sha256`/`size_bytes` are computed over the DECODED bytes by [`ArtifactStore::store`].
    pub fn from_base64_image(
        session_id: SessionId,
        event_id: EventId,
        declared_mime: impl Into<String>,
        base64_data: &str,
    ) -> Result<Self, ArtifactStoreError> {
        use base64::Engine as _;
        let declared = declared_mime.into();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64_data.trim().as_bytes())
            .map_err(|err| ArtifactStoreError::InvalidImageSource {
                detail: format!("base64 decode failed: {err}"),
            })?;
        // § "validar MIME real contra MIME declarado": sniff magic bytes; fall back to
        // the declared MIME when the format is not recognized.
        let decoded = sniff_image_mime(&bytes).unwrap_or_else(|| declared.clone());
        let mut request = Self::new(session_id, event_id, decoded.clone(), Bytes::from(bytes));
        request.source_encoding = Some("base64".to_string());
        request.declared_mime = Some(declared);
        request.decoded_mime = Some(decoded);
        request.availability = ArtifactSourceAvailability::Stored;
        Ok(request)
    }
}

/// Detect a common image MIME from leading magic bytes (§ "validar MIME real").
pub(crate) fn sniff_image_mime(bytes: &[u8]) -> Option<String> {
    let mime = if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };
    Some(mime.to_string())
}

/// Result of storing artifact content.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredArtifact {
    pub metadata: ArtifactMetadata,
    pub storage_mode: ArtifactStorageMode,
    pub inline_content: Option<Bytes>,
}

impl StoredArtifact {
    pub fn is_claim_check(&self) -> bool {
        matches!(self.storage_mode, ArtifactStorageMode::ClaimCheck)
    }

    pub fn to_artifact_ref(&self) -> ArtifactRef {
        ArtifactRef {
            artifact_id: self.metadata.artifact_id.clone(),
            sha256: self.metadata.sha256.clone(),
            size_bytes: self.metadata.size_bytes,
            mime: self.metadata.mime.clone(),
            preview: self.metadata.preview.clone(),
            truncated: self.metadata.truncated,
            ..ArtifactRef::default()
        }
    }

    pub fn to_tool_call_result(&self) -> ToolCallResult {
        if self.is_claim_check() {
            ToolCallResult {
                kind: Some(self.to_artifact_ref().into()),
                ..ToolCallResult::default()
            }
        } else {
            ToolCallResult {
                kind: Some(
                    TextToolResult {
                        content: String::from_utf8_lossy(self.inline_content.as_ref().unwrap()).into_owned(),
                        truncated: false,
                        ..TextToolResult::default()
                    }
                    .into(),
                ),
                ..ToolCallResult::default()
            }
        }
    }
}

/// Retrieved artifact bytes verified against metadata checksum.
#[derive(Clone, Debug, PartialEq)]
pub struct RetrievedArtifact {
    pub metadata: ArtifactMetadata,
    pub content: Bytes,
}

/// Why a referenced artifact could not be produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactUnavailableReason {
    /// The artifact's `storage_ref` is absent or unparseable.
    MissingStorageRef,
    /// The object is not retrievable from the object store (GC'd, expired, or
    /// never persisted).
    NotInObjectStore,
    /// The retrieved bytes failed checksum verification (corrupted).
    ChecksumMismatch,
}

impl ArtifactUnavailableReason {
    /// Stable, low-cardinality label for telemetry / UX.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingStorageRef => "missing_storage_ref",
            Self::NotInObjectStore => "not_in_object_store",
            Self::ChecksumMismatch => "checksum_mismatch",
        }
    }
}

/// A referenced artifact that exists in the canonical session but cannot be
/// retrieved. Surfaced explicitly (Error UX Policy) instead of silently dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactUnavailable {
    pub artifact_id: ArtifactId,
    pub reason: ArtifactUnavailableReason,
}

/// Outcome of retrieving a referenced artifact: either the verified bytes or an
/// explicit `artifact_unavailable` state (`cambio-modelo.md`: a referenced
/// artifact either exists or produces `artifact_unavailable`).
#[derive(Clone, Debug, PartialEq)]
pub enum ArtifactAvailability {
    /// Boxed to keep the enum small: `RetrievedArtifact` carries the full payload.
    Available(Box<RetrievedArtifact>),
    Unavailable(ArtifactUnavailable),
}

/// Claim-check artifact store backed by NATS Object Store.
#[derive(Clone, Debug)]
pub struct ArtifactStore<S> {
    object_store: S,
    config: ArtifactStoreConfig,
}

impl<S> ArtifactStore<S> {
    pub fn new(object_store: S, config: ArtifactStoreConfig) -> Self {
        Self {
            object_store,
            config,
        }
    }

    pub fn config(&self) -> &ArtifactStoreConfig {
        &self.config
    }
}

impl<S> ArtifactStore<S>
where
    S: ObjectStoreDelete + Clone + Send + Sync + 'static,
{
    /// Physically remove a claim-check artifact's object-store blob. Inline
    /// artifacts carry no blob, so deletion is a no-op for them
    /// (§ Event Log Compaction and Retention).
    pub async fn delete(&self, metadata: &ArtifactMetadata) -> Result<(), ArtifactStoreError> {
        if let Some(object_key) = object_key_from_storage_ref(&metadata.storage_ref) {
            self.object_store.delete(object_key).await.map_err(|err| {
                ArtifactStoreError::ObjectStoreDelete {
                    artifact_id: metadata.artifact_id.clone(),
                    detail: err.to_string(),
                }
            })?;
        }
        Ok(())
    }
}

impl<S> ArtifactStore<S>
where
    S: ObjectStorePut + ObjectStoreGet + Clone + Send + Sync + 'static,
{
    pub async fn store(&self, request: StoreArtifactRequest) -> Result<StoredArtifact, ArtifactStoreError> {
        let artifact_id = new_artifact_id();
        let sha256 = sha256_hex(&request.content);
        let (preview, preview_truncated) = build_preview(&request.content, self.config.preview_max_bytes);
        let size_bytes = request.content.len() as u64;
        let created_at = now_timestamp();

        if request.content.len() <= self.config.inline_limit_bytes {
            let metadata = ArtifactMetadata {
                schema_version: SCHEMA_VERSION_V1,
                artifact_id: artifact_id.as_str().to_string(),
                session_id: request.session_id.as_str().to_string(),
                event_id: request.event_id.as_str().to_string(),
                tool_execution_id: request
                    .tool_execution_id
                    .as_ref()
                    .map(|id| id.as_str().to_string()),
                sha256,
                size_bytes,
                mime: request.mime,
                preview,
                storage_ref: String::new(),
                created_at: MessageField::some(created_at),
                retention_policy: EnumValue::Known(request.retention_policy),
                permission_scope: request.permission_scope,
                encryption_status: EnumValue::Known(request.encryption_status),
                truncated: preview_truncated,
                availability: EnumValue::Known(request.availability),
                source_url: request.source_url.clone(),
                fetched_at: request.fetched_at.clone().map(MessageField::some).unwrap_or_default(),
                source_encoding: request.source_encoding.clone(),
                declared_mime: request.declared_mime.clone(),
                decoded_mime: request.decoded_mime.clone(),
                ..ArtifactMetadata::default()
            };

            telemetry::metrics::record_artifact_stored(
                request.session_id.as_str(),
                "inline",
                size_bytes,
            );

            return Ok(StoredArtifact {
                metadata,
                storage_mode: ArtifactStorageMode::Inline,
                inline_content: Some(request.content),
            });
        }

        let object_key = artifact_object_key(&request.session_id, &artifact_id);
        let storage_ref = artifact_storage_ref(&self.config.bucket_name, &object_key);
        let mut reader = Cursor::new(request.content.clone());
        self.object_store
            .put(&object_key, &mut reader)
            .await
            .map_err(|err| ArtifactStoreError::ObjectStorePut {
                session_id: request.session_id.clone(),
                detail: err.to_string(),
            })?;

        let metadata = ArtifactMetadata {
            schema_version: SCHEMA_VERSION_V1,
            artifact_id: artifact_id.as_str().to_string(),
            session_id: request.session_id.as_str().to_string(),
            event_id: request.event_id.as_str().to_string(),
            tool_execution_id: request
                .tool_execution_id
                .as_ref()
                .map(|id| id.as_str().to_string()),
            sha256,
            size_bytes,
            mime: request.mime,
            preview,
            storage_ref,
            created_at: MessageField::some(created_at),
            retention_policy: EnumValue::Known(request.retention_policy),
            permission_scope: request.permission_scope,
            encryption_status: EnumValue::Known(request.encryption_status),
            truncated: true,
            availability: EnumValue::Known(request.availability),
            source_url: request.source_url.clone(),
            fetched_at: request.fetched_at.clone().map(MessageField::some).unwrap_or_default(),
            source_encoding: request.source_encoding.clone(),
            declared_mime: request.declared_mime.clone(),
            decoded_mime: request.decoded_mime.clone(),
            ..ArtifactMetadata::default()
        };

        telemetry::metrics::record_artifact_stored(
            request.session_id.as_str(),
            "claim_check",
            size_bytes,
        );

        Ok(StoredArtifact {
            metadata,
            storage_mode: ArtifactStorageMode::ClaimCheck,
            inline_content: None,
        })
    }

    pub async fn retrieve(&self, metadata: &ArtifactMetadata) -> Result<RetrievedArtifact, ArtifactStoreError> {
        let artifact_id = ArtifactId::new(&metadata.artifact_id).map_err(|err| {
            ArtifactStoreError::ObjectStoreGet {
                artifact_id: ArtifactId::new("artifact_invalid").expect("valid artifact id"),
                detail: err.to_string(),
            }
        })?;

        if metadata.storage_ref.is_empty() {
            return Err(ArtifactStoreError::InlineArtifact { artifact_id });
        }

        let object_key = object_key_from_storage_ref(&metadata.storage_ref).ok_or_else(|| {
            ArtifactStoreError::MissingStorageRef {
                artifact_id: artifact_id.clone(),
            }
        })?;

        let mut reader = self
            .object_store
            .get(object_key)
            .await
            .map_err(|err| ArtifactStoreError::ObjectStoreGet {
                artifact_id: artifact_id.clone(),
                detail: err.to_string(),
            })?;

        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf)
            .await
            .map_err(ArtifactStoreError::Read)?;

        let content = Bytes::from(buf);
        let actual = sha256_hex(&content);
        if actual != metadata.sha256 {
            telemetry::metrics::record_checksum_mismatch(metadata.session_id.as_str(), metadata.artifact_id.as_str());
            return Err(ArtifactStoreError::ChecksumMismatch {
                artifact_id,
                expected: metadata.sha256.clone(),
                actual,
            });
        }

        telemetry::metrics::record_artifact_retrieved(metadata.session_id.as_str(), metadata.artifact_id.as_str());
        Ok(RetrievedArtifact {
            metadata: metadata.clone(),
            content,
        })
    }

    /// Retrieve a referenced artifact, mapping "the bytes aren't there or can't be
    /// trusted" to an explicit [`ArtifactAvailability::Unavailable`] state instead
    /// of a hard error (Error UX Policy). Inline artifacts and transient read
    /// errors stay `Err` (caller misuse / retryable), never silent unavailability.
    pub async fn retrieve_availability(
        &self,
        metadata: &ArtifactMetadata,
    ) -> Result<ArtifactAvailability, ArtifactStoreError> {
        let unavailable = |artifact_id: ArtifactId, reason: ArtifactUnavailableReason| {
            telemetry::metrics::record_artifact_unavailable(
                metadata.session_id.as_str(),
                metadata.artifact_id.as_str(),
                reason.as_str(),
            );
            Ok(ArtifactAvailability::Unavailable(ArtifactUnavailable {
                artifact_id,
                reason,
            }))
        };
        match self.retrieve(metadata).await {
            Ok(retrieved) => Ok(ArtifactAvailability::Available(Box::new(retrieved))),
            Err(ArtifactStoreError::MissingStorageRef { artifact_id }) => {
                unavailable(artifact_id, ArtifactUnavailableReason::MissingStorageRef)
            }
            Err(ArtifactStoreError::ObjectStoreGet { artifact_id, .. }) => {
                unavailable(artifact_id, ArtifactUnavailableReason::NotInObjectStore)
            }
            Err(ArtifactStoreError::ChecksumMismatch { artifact_id, .. }) => {
                unavailable(artifact_id, ArtifactUnavailableReason::ChecksumMismatch)
            }
            Err(other) => Err(other),
        }
    }

    pub async fn retrieve_by_ref(
        &self,
        session_id: &SessionId,
        artifact_ref: &ArtifactRef,
    ) -> Result<RetrievedArtifact, ArtifactStoreError> {
        let artifact_id = ArtifactId::new(&artifact_ref.artifact_id).map_err(|err| {
            ArtifactStoreError::ObjectStoreGet {
                artifact_id: ArtifactId::new("artifact_invalid").expect("valid artifact id"),
                detail: err.to_string(),
            }
        })?;
        let object_key = artifact_object_key(session_id, &artifact_id);
        let metadata = ArtifactMetadata {
            schema_version: SCHEMA_VERSION_V1,
            artifact_id: artifact_ref.artifact_id.clone(),
            session_id: session_id.as_str().to_string(),
            sha256: artifact_ref.sha256.clone(),
            size_bytes: artifact_ref.size_bytes,
            mime: artifact_ref.mime.clone(),
            preview: artifact_ref.preview.clone(),
            truncated: artifact_ref.truncated,
            storage_ref: artifact_storage_ref(&self.config.bucket_name, &object_key),
            ..ArtifactMetadata::default()
        };

        self.retrieve(&metadata).await
    }
}

fn new_artifact_id() -> ArtifactId {
    ArtifactId::new(format!("artifact_{}", uuid::Uuid::now_v7())).expect("generated artifact id is valid")
}

fn now_timestamp() -> buffa_types::google::protobuf::Timestamp {
    let now = OffsetDateTime::now_utc();
    buffa_types::google::protobuf::Timestamp {
        seconds: now.unix_timestamp(),
        nanos: now.nanosecond() as i32,
        ..buffa_types::google::protobuf::Timestamp::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_nats::jetstream::MockObjectStore;

    fn test_store(inline_limit: usize) -> ArtifactStore<MockObjectStore> {
        ArtifactStore::new(
            MockObjectStore::new(),
            ArtifactStoreConfig {
                inline_limit_bytes: inline_limit,
                preview_max_bytes: 64,
                bucket_name: "ACP_SESSION_ARTIFACTS".to_string(),
                permission_scope: "workspace:default".to_string(),
            },
        )
    }

    #[tokio::test]
    async fn small_payload_stays_inline_without_object_store_write() {
        let store = test_store(1024);
        let session_id = SessionId::new("sess_inline").unwrap();
        let event_id = EventId::new("evt_inline").unwrap();
        let content = Bytes::from_static(b"small tool output");

        let stored = store
            .store(StoreArtifactRequest::new(
                session_id,
                event_id,
                "text/plain",
                content.clone(),
            ))
            .await
            .unwrap();

        assert_eq!(stored.storage_mode, ArtifactStorageMode::Inline);
        assert_eq!(stored.inline_content, Some(content));
        assert!(stored.metadata.storage_ref.is_empty());
        assert!(store.object_store.stored_objects().is_empty());
    }

    #[tokio::test]
    async fn large_payload_uses_claim_check_and_can_be_retrieved() {
        let store = test_store(32);
        let session_id = SessionId::new("sess_claim").unwrap();
        let event_id = EventId::new("evt_claim").unwrap();
        let content = Bytes::from("large ".repeat(20));

        let stored = store
            .store(StoreArtifactRequest::new(
                session_id.clone(),
                event_id,
                "text/plain",
                content.clone(),
            ))
            .await
            .unwrap();

        assert_eq!(stored.storage_mode, ArtifactStorageMode::ClaimCheck);
        assert!(stored.is_claim_check());
        assert!(stored.metadata.storage_ref.starts_with("obj://ACP_SESSION_ARTIFACTS/"));
        assert!(stored.metadata.truncated);
        assert_eq!(store.object_store.stored_objects().len(), 1);

        let retrieved = store.retrieve(&stored.metadata).await.unwrap();
        assert_eq!(retrieved.content, content);
        assert_eq!(retrieved.metadata.sha256, stored.metadata.sha256);
    }

    #[tokio::test]
    async fn retrieve_verifies_checksum() {
        let store = test_store(16);
        let session_id = SessionId::new("sess_checksum").unwrap();
        let event_id = EventId::new("evt_checksum").unwrap();
        let content = Bytes::from("checksum payload data");

        let stored = store
            .store(StoreArtifactRequest::new(
                session_id,
                event_id,
                "text/plain",
                content,
            ))
            .await
            .unwrap();

        let mut corrupted = stored.metadata.clone();
        corrupted.sha256 = "deadbeef".to_string();
        let err = store.retrieve(&corrupted).await.unwrap_err();
        assert!(matches!(err, ArtifactStoreError::ChecksumMismatch { .. }));
    }

    #[test]
    fn claim_check_tool_result_uses_artifact_ref() {
        let stored = StoredArtifact {
            metadata: ArtifactMetadata {
                artifact_id: "artifact_test".to_string(),
                sha256: "abc".to_string(),
                size_bytes: 100,
                mime: "text/plain".to_string(),
                preview: "preview".to_string(),
                truncated: true,
                ..ArtifactMetadata::default()
            },
            storage_mode: ArtifactStorageMode::ClaimCheck,
            inline_content: None,
        };

        let artifact_ref = stored.to_artifact_ref();
        assert_eq!(artifact_ref.artifact_id, "artifact_test");
        assert!(artifact_ref.truncated);
    }

    // § Imagen Base64: decode -> validate -> hash -> store bytes as artifact; keep
    // source_encoding/declared_mime/decoded_mime as metadata; never store the string.

    fn png_bytes() -> Vec<u8> {
        // PNG signature + a few payload bytes.
        vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x01, 0x02, 0x03]
    }

    #[test]
    fn from_base64_image_decodes_to_raw_bytes_and_validates_mime() {
        use base64::Engine as _;
        let png = png_bytes();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        // Declared MIME is deliberately wrong; the real MIME is sniffed from the bytes.
        let request = StoreArtifactRequest::from_base64_image(
            SessionId::new("sess_img").unwrap(),
            EventId::new("evt_img").unwrap(),
            "image/jpeg",
            &b64,
        )
        .unwrap();

        // Content is the DECODED bytes, not the Base64 string (§ "no guardar el string").
        assert_eq!(request.content.as_ref(), png.as_slice());
        assert_eq!(request.source_encoding.as_deref(), Some("base64"));
        assert_eq!(request.declared_mime.as_deref(), Some("image/jpeg"));
        assert_eq!(request.decoded_mime.as_deref(), Some("image/png"));
        assert_eq!(request.mime, "image/png");
        assert_eq!(request.availability, ArtifactSourceAvailability::Stored);
    }

    #[test]
    fn from_base64_image_rejects_undecodable_source() {
        let err = StoreArtifactRequest::from_base64_image(
            SessionId::new("sess_img").unwrap(),
            EventId::new("evt_img").unwrap(),
            "image/png",
            "!!! not base64 !!!",
        )
        .unwrap_err();
        assert!(matches!(err, ArtifactStoreError::InvalidImageSource { .. }));
    }

    #[tokio::test]
    async fn stored_base64_image_hashes_decoded_bytes_not_the_string() {
        use base64::Engine as _;
        let png = png_bytes();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let request = StoreArtifactRequest::from_base64_image(
            SessionId::new("sess_img").unwrap(),
            EventId::new("evt_img").unwrap(),
            "image/png",
            &b64,
        )
        .unwrap();

        let store = test_store(1024);
        let stored = store.store(request).await.unwrap();

        // sha256 is over the decoded image bytes, not the Base64 string (§ "sha256 sobre
        // los bytes, no sobre el string Base64").
        assert_eq!(stored.metadata.sha256, crate::checksum::sha256_hex(&Bytes::from(png.clone())));
        assert_eq!(stored.metadata.size_bytes, png.len() as u64);
        assert_eq!(stored.metadata.source_encoding.as_deref(), Some("base64"));
        assert_eq!(stored.metadata.decoded_mime.as_deref(), Some("image/png"));
        assert_eq!(
            stored.metadata.availability.as_known(),
            Some(ArtifactSourceAvailability::Stored)
        );
    }
}
