//! The micro error channel (ADR 0016 §3): a reply is an error iff
//! `Nats-Service-Error-Code` is present, and on error the body is one
//! complete `google.rpc.Status` encoded per the negotiated [`ContentType`].

use async_nats::HeaderMap;
use bytes::Bytes;
use thiserror::Error;
use trogonai_proto::google::rpc::Status;

use crate::constants::{HEADER_CONTENT_TYPE, HEADER_ERROR, HEADER_ERROR_CODE};
use crate::content_type::{ContentType, DecodeError, EncodeError};
use crate::content_type_input::ContentTypeInput;
use crate::service_error_code::{ServiceErrorCode, ServiceErrorCodeError};
use crate::service_error_code_input::ServiceErrorCodeInput;
use crate::service_fault::ServiceFault;

/// A successful reply body, or a fault reported on the micro error channel.
pub enum Outcome {
    Success(Bytes),
    Error(ServiceFault),
}

/// Headers and body ready to publish as a NATS reply.
pub struct EncodedReply {
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// Server-side: encode an [`Outcome`] into the headers and body a NATS reply
/// needs, per ADR 0016 §3.
pub fn encode_reply(outcome: Outcome, content_type: ContentType) -> Result<EncodedReply, EncodeError> {
    match outcome {
        Outcome::Success(body) => Ok(EncodedReply {
            headers: HeaderMap::new(),
            body,
        }),
        Outcome::Error(fault) => {
            let body = content_type.encode(fault.status())?;
            let mut headers = HeaderMap::new();
            headers.insert(HEADER_ERROR, fault.message());
            headers.insert(HEADER_ERROR_CODE, fault.code().to_i32().to_string().as_str());
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
/// readable.
#[derive(Debug, Clone, PartialEq, Error)]
#[error("nats micro service error ({}): {}", .fault.code(), .fault.message())]
pub struct ServiceError {
    fault: ServiceFault,
}

impl ServiceError {
    pub const fn code(&self) -> ServiceErrorCode {
        self.fault.code()
    }

    pub fn message(&self) -> &str {
        self.fault.message()
    }

    pub fn status(&self) -> &Status {
        self.fault.status()
    }

    pub fn into_status(self) -> Status {
        self.fault.into_status()
    }
}

/// Client-side: decode a raw NATS reply per the ADR 0016 §3 error-channel rule.
///
/// A reply is an error iff [`HEADER_ERROR_CODE`] is present; the header value
/// is the canonical `google.rpc.Code`, authoritative over the body's `code` field on
/// disagreement, and the body is decoded as the complete [`Status`]. Absent
/// the header, the body is decoded as `Resp`.
///
/// `requested` is only a fallback for a reply that declares no `Content-Type`:
/// ADR 0016 §4 makes the reply's own `Content-Type` authoritative for how its
/// body is encoded, which is what lets a rejection of the requested encoding
/// still be readable. A reply that declares an encoding this binding does not
/// speak is reported rather than decoded, since falling back to `requested`
/// there would decode the body as something the sender never wrote.
pub fn decode_reply<Resp>(headers: Option<&HeaderMap>, body: &[u8], requested: ContentType) -> Result<Resp, ReplyError>
where
    Resp: buffa::Message + serde::de::DeserializeOwned,
{
    let declared = headers
        .and_then(|headers| headers.get(HEADER_CONTENT_TYPE))
        .map(|value| ContentTypeInput::new(value.as_str()));
    let content_type = match declared {
        Some(declared) => ContentType::from_input(&declared).ok_or(ReplyError::ContentType { declared })?,
        None => requested,
    };
    let error_code = headers.and_then(|headers| headers.get(HEADER_ERROR_CODE));

    match error_code {
        Some(code_header) => {
            let input = ServiceErrorCodeInput::new(code_header.as_str());
            let code = ServiceErrorCode::from_input(&input).map_err(ReplyError::ErrorCode)?;
            let status: Status = content_type.decode(body).map_err(ReplyError::Decode)?;
            Err(ReplyError::Service(ServiceError {
                fault: ServiceFault::with_code(code, status),
            }))
        }
        None => content_type.decode(body).map_err(ReplyError::Decode),
    }
}

#[derive(Debug, Error)]
pub enum ReplyError {
    #[error("invalid {HEADER_ERROR_CODE} header")]
    ErrorCode(#[source] ServiceErrorCodeError),
    #[error("reply declares an unsupported Content-Type: {declared}")]
    ContentType { declared: ContentTypeInput },
    #[error("failed to decode reply payload")]
    Decode(#[source] DecodeError),
    #[error(transparent)]
    Service(#[from] ServiceError),
}

#[cfg(test)]
mod tests;
