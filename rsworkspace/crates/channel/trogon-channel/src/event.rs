#[cfg(test)]
#[path = "event_tests.rs"]
mod event_tests;

use crate::command::Command;
use crate::endpoint::{Endpoint, EndpointError};
use crate::safe_token::SafeToken;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EventFieldError {
    #[error("a message reference may not be blank")]
    BlankMessageRef,
    #[error(transparent)]
    NotAMediaType(#[from] MediaTypeError),
}

/// Who sent a message, in the sending platform's own terms. This is an
/// identity, not an address: it says nothing about where a reply goes, which is
/// what [`Endpoint`] is for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sender {
    pub platform_user_id: PlatformUserId,
    /// Free-form, as the platform renders it. Carries no invariant on purpose:
    /// it exists to be shown to the agent and never to be matched on.
    pub display_name: String,
}

/// The sender's id on its platform. Constrained to an endpoint token because
/// that is what it becomes: a sender is authorized by building the endpoint
/// `{channel}.{account}.{platform_user_id}` and looking up its principal, so an
/// id that cannot be a token could never be authorized anyway.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PlatformUserId(SafeToken);

impl PlatformUserId {
    pub fn new(id: impl Into<String>) -> Result<Self, EndpointError> {
        Ok(Self(SafeToken::new(id)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// A platform that numbers its users hands the id over as an integer, and every
/// integer is already a token, so this path has no failure to report.
impl From<u64> for PlatformUserId {
    fn from(id: u64) -> Self {
        Self(SafeToken::from(id))
    }
}

impl std::fmt::Display for PlatformUserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PlatformUserId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// A platform's own identifier for one message, used to recognize a message the
/// bridge has already handled and to address edits and reactions back at it.
/// Opaque: only equality and round-tripping are ever asked of it, so the single
/// invariant is that it is not blank. Deliberately looser than an endpoint
/// token, because message ids elsewhere are not tokens (an email `Message-ID`
/// carries `@` and `.`) and this type is channel-neutral.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct MessageRef(String);

impl MessageRef {
    pub fn new(reference: impl Into<String>) -> Result<Self, EventFieldError> {
        let reference = reference.into();
        if reference.trim().is_empty() {
            return Err(EventFieldError::BlankMessageRef);
        }
        Ok(Self(reference))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A platform that numbers its messages hands the id over as an integer, whose
/// decimal form is never blank, so this path has no failure to report either.
impl From<i64> for MessageRef {
    fn from(id: i64) -> Self {
        Self(id.to_string())
    }
}

impl std::fmt::Display for MessageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for MessageRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// What kind of media arrived, in the channel-neutral vocabulary. A closed set
/// for the same reason [`crate::RenderCommand`] is one: a bridge must be able to
/// render every kind, so a kind no bridge knows is not a kind. A platform
/// distinction this cannot express (a Telegram voice note versus an audio file)
/// is either mapped onto the nearest kind or carried in agent `_meta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Video,
    Audio,
    /// Speech recorded in the client, which platforms treat as its own kind
    /// because it is transcribable rather than merely playable.
    Voice,
    Document,
}

/// Which rule a would-be media type broke. Named reasons rather than a copy of
/// the input: the rejected text belongs to whatever log records the rejection,
/// and a caller matching on why can tell a missing subtype from a stray space.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MediaTypeError {
    #[error("a media type needs a type and a subtype separated by '/'")]
    MissingSeparator,
    #[error("a media type's type may not be empty")]
    EmptyType,
    #[error("a media type's subtype may not be empty")]
    EmptySubtype,
    #[error("a media type has one subtype, so its subtype may not contain '/'")]
    SubtypeIsNotOne,
    #[error("a media type's type and subtype may not contain whitespace")]
    InteriorWhitespace,
}

/// An IANA media type, whose type and subtype are normalized to lower case
/// because the standard defines those two as case-insensitive and a caller
/// comparing them as bytes would otherwise be wrong for `IMAGE/PNG`.
///
/// Parameters are kept byte for byte apart from the optional space around the
/// separator, because case-insensitivity stops at the subtype: a `multipart`
/// boundary and a `filename` are values a sender chose and folding them changes
/// what they refer to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct MimeType(String);

impl MimeType {
    pub fn new(raw: impl Into<String>) -> Result<Self, EventFieldError> {
        let raw = raw.into();
        let trimmed = raw.trim();
        // The space either side of the separator is optional in the standard, so
        // it says nothing about which media type this is and is dropped here
        // rather than left to make one spelling of a type unequal to another.
        let (essence, parameters) = match trimmed.split_once(';') {
            Some((essence, parameters)) => (essence.trim_end(), Some(parameters.trim_start())),
            None => (trimmed, None),
        };
        let (kind, subtype) = essence.split_once('/').ok_or(MediaTypeError::MissingSeparator)?;
        if kind.is_empty() {
            return Err(MediaTypeError::EmptyType.into());
        }
        if subtype.is_empty() {
            return Err(MediaTypeError::EmptySubtype.into());
        }
        if subtype.contains('/') {
            return Err(MediaTypeError::SubtypeIsNotOne.into());
        }
        // Only the type and subtype. A parameter value is the sender's to choose
        // and a quoted one may hold spaces, so what is inside the parameters is
        // not this constructor's to reject.
        if essence.chars().any(char::is_whitespace) {
            return Err(MediaTypeError::InteriorWhitespace.into());
        }
        let mut normalized = format!("{}/{}", kind.to_ascii_lowercase(), subtype.to_ascii_lowercase());
        if let Some(parameters) = parameters {
            normalized.push(';');
            normalized.push_str(parameters);
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MimeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for MimeType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// The platform's handle for a file, redeemable for bytes only by the channel
/// that issued it (e.g. a Telegram `file_id`). Constrained to an endpoint token
/// because it is also a KV key: readiness for the fetch lives at this handle in
/// `channel_media_{prefix}`, so a handle that is not a safe key has nowhere to
/// report. See ADR#0044.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PlatformRef(SafeToken);

impl PlatformRef {
    pub fn new(reference: impl Into<String>) -> Result<Self, EndpointError> {
        Ok(Self(SafeToken::new(reference)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// KV key for this handle's readiness record in `channel_media_{prefix}`.
    pub fn kv_key(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for PlatformRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PlatformRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// Media that arrived with a message, as a handle rather than as bytes.
/// `platform_ref` is the platform's own reference (e.g. a Telegram `file_id`);
/// redeeming it happens out of band, so this type never asserts that bytes
/// exist yet. See ADR#0044.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub mime: MimeType,
    /// Bytes, as the platform reports them before the fetch. Advisory: the
    /// downloader reports the size it actually stored.
    pub size: u64,
    pub platform_ref: PlatformRef,
}

/// A normalized inbound message: what any channel bridge produces after
/// stripping its platform's shape. Travels in process; `Serialize` because the
/// shape is the cross-channel contract, not because anything publishes it.
///
/// Every field that carries an invariant is a value object that enforces it at
/// construction, which is also what makes `Deserialize` safe here: there is no
/// path that turns channel-provided JSON into an unchecked field, so the type
/// needs no separate wire twin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundEvent {
    pub endpoint: Endpoint,
    pub sender: Sender,
    /// Message text with any command trigger already removed, so what reaches
    /// the agent is only what the user meant for it.
    pub text: Option<String>,
    /// A bridge command found in the text. Extracted at the channel edge
    /// because the trigger vocabulary is a channel affordance; acted on by the
    /// routing layer and never forwarded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Command>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Platform message identity, for dedup, replies, and edits.
    pub message_ref: MessageRef,
    /// Unix seconds, as reported by the platform. A bare integer for the same
    /// reason `ConversationRecord::created_at` is: this crate takes no clock,
    /// and the whole crate spells timestamps one way.
    pub occurred_at: i64,
}
