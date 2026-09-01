//! The micro error channel (ADR 0016 §3): a reply is an error iff
//! `Nats-Service-Error-Code` is present, and on error the body is one
//! complete `google.rpc.Status` encoded per the negotiated [`ContentType`].

use async_nats::HeaderMap;
use buffa::Enumeration as _;
use bytes::Bytes;
use thiserror::Error;
use trogonai_proto::google::rpc::{Code, Status};

use crate::constants::{HEADER_CONTENT_TYPE, HEADER_ERROR, HEADER_ERROR_CODE};
use crate::content_type::{ContentType, DecodeError, EncodeError};

/// A successful reply body, or a fault reported on the micro error channel.
pub enum Outcome {
    Success(Bytes),
    Error(Status),
}

/// Headers and body ready to publish as a NATS reply.
pub struct EncodedReply {
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// Server-side: encode an [`Outcome`] into the headers and body a NATS reply
/// needs, per ADR 0016 §3.
///
/// An `Outcome::Error` whose `code` is `OK` (0) is a programmer error: this
/// coerces it to `INTERNAL` rather than emit an error reply with no error
/// code, since the ADR requires the error code space to never contain `OK`
/// on an error reply.
pub fn encode_reply(outcome: Outcome, content_type: ContentType) -> Result<EncodedReply, EncodeError> {
    match outcome {
        Outcome::Success(body) => Ok(EncodedReply {
            headers: HeaderMap::new(),
            body,
        }),
        Outcome::Error(mut status) => {
            if status.code == Code::OK.to_i32() {
                status.code = Code::INTERNAL.to_i32();
            }
            let body = content_type.encode(&status)?;
            let mut headers = HeaderMap::new();
            headers.insert(HEADER_ERROR, status.message.as_str());
            headers.insert(HEADER_ERROR_CODE, status.code.to_string().as_str());
            Ok(EncodedReply {
                headers,
                body: Bytes::from(body),
            })
        }
    }
}

/// A decoded micro service error: the whole `google.rpc.Status` from the reply
/// body, so `details` (`ErrorInfo`, `BadRequest`, `RetryInfo`, ...) reaches the
/// caller, since ADR 0016 §3 makes the body the only place `details` is
/// readable. `code` is reconciled to the `Nats-Service-Error-Code` header,
/// which the same section makes authoritative on disagreement with the body.
#[derive(Debug, Clone, PartialEq, Error)]
#[error("nats micro service error (code {}): {}", .status.code, .status.message)]
pub struct ServiceError {
    status: Status,
}

impl ServiceError {
    fn from_status(header_code: i32, mut status: Status) -> Self {
        status.code = header_code;
        Self { status }
    }

    pub fn code(&self) -> i32 {
        self.status.code
    }

    pub fn message(&self) -> &str {
        &self.status.message
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn into_status(self) -> Status {
        self.status
    }
}

/// Client-side: decode a raw NATS reply per the ADR 0016 §3 error-channel rule.
///
/// A reply is an error iff [`HEADER_ERROR_CODE`] is present; the header value
/// is the canonical [`Code`], authoritative over the body's `code` field on
/// disagreement, and the body is decoded as the complete [`Status`]. Absent
/// the header, the body is decoded as `Resp`.
///
/// `requested` is only a fallback: ADR 0016 §4 makes the reply's own
/// `Content-Type` authoritative for how its body is encoded, which is what
/// lets a rejection of the requested encoding still be readable.
pub fn decode_reply<Resp>(headers: Option<&HeaderMap>, body: &[u8], requested: ContentType) -> Result<Resp, ReplyError>
where
    Resp: buffa::Message + serde::de::DeserializeOwned,
{
    let content_type = headers
        .and_then(|headers| headers.get(HEADER_CONTENT_TYPE))
        .and_then(|value| ContentType::from_header_value(value.as_str()))
        .unwrap_or(requested);
    let error_code = headers.and_then(|headers| headers.get(HEADER_ERROR_CODE));

    match error_code {
        Some(code_header) => {
            let header_code: i32 = code_header
                .as_str()
                .parse()
                .map_err(|_| ReplyError::InvalidErrorCodeHeader {
                    value: code_header.as_str().to_string(),
                })?;
            let status: Status = content_type.decode(body).map_err(ReplyError::Decode)?;
            Err(ReplyError::Service(ServiceError::from_status(header_code, status)))
        }
        None => content_type.decode(body).map_err(ReplyError::Decode),
    }
}

#[derive(Debug, Error)]
pub enum ReplyError {
    #[error("invalid {HEADER_ERROR_CODE} header value: {value}")]
    InvalidErrorCodeHeader { value: String },
    #[error("failed to decode reply payload")]
    Decode(#[source] DecodeError),
    #[error(transparent)]
    Service(#[from] ServiceError),
}
