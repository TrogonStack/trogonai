//! The micro error channel (ADR 0016 §3): a reply is an error iff
//! `Nats-Service-Error-Code` is present, and on error the body is one
//! complete `google.rpc.Status` encoded per the negotiated [`ContentType`].

use async_nats::HeaderMap;
use buffa::Enumeration as _;
use bytes::Bytes;
use thiserror::Error;
use trogonai_proto::google::rpc::{Code, Status};

use crate::constants::{HEADER_ERROR, HEADER_ERROR_CODE};
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

/// A decoded micro service error: the headers are authoritative for `code`
/// on any disagreement with the body per ADR 0016 §3.
#[derive(Debug, Clone, PartialEq, Error)]
#[error("nats micro service error (code {code}): {message}")]
pub struct ServiceError {
    pub code: i32,
    pub message: String,
}

impl ServiceError {
    fn from_status(header_code: i32, status: Status) -> Self {
        Self {
            code: header_code,
            message: status.message,
        }
    }
}

/// Client-side: decode a raw NATS reply per the ADR 0016 §3 error-channel rule.
///
/// A reply is an error iff [`HEADER_ERROR_CODE`] is present; the header value
/// is the canonical [`Code`], authoritative over the body's `code` field on
/// disagreement, and the body is decoded as the complete [`Status`]. Absent
/// the header, the body is decoded as `Resp`.
pub fn decode_reply<Resp>(
    headers: Option<&HeaderMap>,
    body: &[u8],
    content_type: ContentType,
) -> Result<Resp, ReplyError>
where
    Resp: buffa::Message + serde::de::DeserializeOwned,
{
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
