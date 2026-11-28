//! Typed non-text content produced by a tool.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{ContentHash, ReadCoverage, ToolUsage, UnixMillis};

/// Complete text held until a writable session can retain it outside the model request.
#[derive(Clone, PartialEq, Eq)]
pub struct DurableToolText(Arc<String>);

impl DurableToolText {
    /// The largest complete encoded text artifact a session can read back.
    pub const MAX_BYTES: usize = 8 * 1024 * 1024;

    /// Retains complete text within the durable readback bound.
    pub fn new(text: String) -> Result<Self, ToolTextTooLarge> {
        if text.len() > Self::MAX_BYTES {
            return Err(ToolTextTooLarge);
        }
        Ok(Self(Arc::new(text)))
    }

    /// The complete text before its model-facing cap.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for DurableToolText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableToolText")
            .field("bytes", &self.0.len())
            .finish()
    }
}

/// Complete text larger than the session can recover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("tool text exceeds the durable storage bound")]
pub struct ToolTextTooLarge;

/// JSON whose boundary must survive beside a tool's text projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolStructuredContent(Value);

impl ToolStructuredContent {
    /// Parses one complete JSON value.
    pub fn parse(value: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(value).map(Self)
    }

    /// The structured value.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.0
    }
}

/// A resource a tool referenced without embedding its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolLinkedResource {
    uri: String,
    media_type: Option<String>,
    text: Option<String>,
}

impl<'de> Deserialize<'de> for ToolLinkedResource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            uri: String,
            media_type: Option<String>,
            text: Option<String>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.uri, wire.media_type, wire.text).map_err(serde::de::Error::custom)
    }
}

impl ToolLinkedResource {
    /// Builds a usable linked resource.
    pub fn new(
        uri: impl Into<String>,
        media_type: Option<String>,
        text: Option<String>,
    ) -> Result<Self, InvalidToolLinkedResource> {
        let uri = uri.into();
        if uri.is_empty() || uri.chars().any(char::is_control) {
            return Err(InvalidToolLinkedResource::InvalidUri);
        }
        if media_type.as_deref().is_some_and(str::is_empty) {
            return Err(InvalidToolLinkedResource::EmptyMediaType);
        }
        Ok(Self { uri, media_type, text })
    }

    /// The resource identifier.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// The declared media type, when the tool supplied one.
    #[must_use]
    pub fn media_type(&self) -> Option<&str> {
        self.media_type.as_deref()
    }

    /// The resource's text projection, when the tool supplied one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

/// Why a linked resource could not become a domain value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InvalidToolLinkedResource {
    /// An empty or control-bearing identifier cannot be recovered or followed.
    #[error("linked tool resource URI is invalid")]
    InvalidUri,
    /// An empty media type is different from not declaring one.
    #[error("linked tool resource media type is empty")]
    EmptyMediaType,
}

/// One non-text boundary retained from a rich tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ToolOutputFragment {
    /// A structured JSON value.
    Json(ToolStructuredContent),
    /// A resource reference with its optional text projection.
    LinkedResource(ToolLinkedResource),
}

/// Durable evidence reported by a tool that read a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReadEvidence {
    /// The path the tool resolved and read.
    pub path: std::path::PathBuf,
    /// The host-reported modification time, when it was representable.
    pub modified: Option<UnixMillis>,
    /// The digest of the bytes the tool read.
    pub content_hash: ContentHash,
}

/// Operational facts a tool result carries beside its model-facing content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultObservations {
    /// Paths the call created, changed, or removed.
    #[serde(default)]
    pub touched_paths: Vec<std::path::PathBuf>,
    /// How much of a file the model was shown.
    #[serde(default)]
    pub coverage: Option<ReadCoverage>,
    /// The file read that produced the evidence.
    #[serde(default)]
    pub read: Option<ToolReadEvidence>,
    /// Usage reported by a provider-run tool.
    #[serde(default)]
    pub usage: Option<ToolUsage>,
}
