use buffa::Message;
use thiserror::Error;
use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};

use crate::constants::{CONTENT_TYPE_JSON, CONTENT_TYPE_PROTOBUF};
use crate::content_type_input::ContentTypeInput;

/// The wire encoding used for a request or reply payload (ADR 0016 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Protobuf,
    Json,
}

impl ContentType {
    /// The set of `Content-Type` values a [`ServiceOptions::content_type`]
    /// restriction allows on the wire.
    fn allowed(policy: &ServiceOptions) -> Allowed {
        match policy.content_type.as_known() {
            Some(ProtoContentType::CONTENT_TYPE_PROTOBUF) => Allowed::Only(Self::Protobuf),
            Some(ProtoContentType::CONTENT_TYPE_JSON) => Allowed::Only(Self::Json),
            Some(ProtoContentType::CONTENT_TYPE_UNSPECIFIED) | None => Allowed::Either,
        }
    }

    /// The encoding a `Content-Type` header names, or `None` if the value is
    /// not one this binding speaks (ADR 0016 §4). The one conversion from the
    /// wire value into this domain value.
    pub fn from_input(input: &ContentTypeInput) -> Option<Self> {
        match input.as_str() {
            CONTENT_TYPE_PROTOBUF => Some(Self::Protobuf),
            CONTENT_TYPE_JSON => Some(Self::Json),
            _ => None,
        }
    }

    /// Negotiate the [`ContentType`] for a request given the service's
    /// [`ServiceOptions`] restriction and the request's `Content-Type` header
    /// value, if any.
    ///
    /// An absent header accepts either type allowed by `policy`; on ambiguity
    /// (no header and both types allowed) this defaults to [`Self::Protobuf`].
    pub fn negotiate(policy: &ServiceOptions, header: Option<&ContentTypeInput>) -> Result<Self, NegotiationError> {
        let allowed = Self::allowed(policy);
        match header {
            Some(input) => {
                let requested = Self::from_input(input).ok_or_else(|| NegotiationError::Unsupported {
                    requested: input.clone(),
                })?;
                match allowed {
                    Allowed::Either => Ok(requested),
                    Allowed::Only(only) if only == requested => Ok(requested),
                    Allowed::Only(_) => Err(NegotiationError::NotAllowed { requested }),
                }
            }
            None => match allowed {
                Allowed::Either => Ok(Self::Protobuf),
                Allowed::Only(content_type) => Ok(content_type),
            },
        }
    }

    /// The `Content-Type` header value for this encoding.
    pub const fn header_value(self) -> &'static str {
        match self {
            Self::Protobuf => CONTENT_TYPE_PROTOBUF,
            Self::Json => CONTENT_TYPE_JSON,
        }
    }

    /// Encode a protobuf message per this content type.
    pub fn encode<M>(self, message: &M) -> Result<Vec<u8>, EncodeError>
    where
        M: Message + serde::Serialize,
    {
        match self {
            Self::Protobuf => Ok(message.encode_to_vec()),
            Self::Json => serde_json::to_vec(message).map_err(EncodeError::Json),
        }
    }

    /// Decode a protobuf message per this content type.
    pub fn decode<M>(self, bytes: &[u8]) -> Result<M, DecodeError>
    where
        M: Message + serde::de::DeserializeOwned,
    {
        match self {
            Self::Protobuf => M::decode_from_slice(bytes).map_err(DecodeError::Protobuf),
            Self::Json => serde_json::from_slice(bytes).map_err(DecodeError::Json),
        }
    }
}

enum Allowed {
    Either,
    Only(ContentType),
}

#[derive(Debug, Error)]
pub enum NegotiationError {
    #[error("content type {requested:?} is not allowed by the service's content-type policy")]
    NotAllowed { requested: ContentType },
    #[error("unrecognized Content-Type header value: {requested}")]
    Unsupported { requested: ContentTypeInput },
}

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("failed to encode payload as JSON")]
    Json(#[source] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("failed to decode payload as protobuf")]
    Protobuf(#[source] buffa::DecodeError),
    #[error("failed to decode payload as JSON")]
    Json(#[source] serde_json::Error),
}
