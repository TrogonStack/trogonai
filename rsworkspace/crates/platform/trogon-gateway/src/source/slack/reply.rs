use async_nats::Message;
use axum::http::{HeaderValue, StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde_json::value::RawValue;

use super::constants::{CONTENT_TYPE_JSON, CONTENT_TYPE_TEXT, NATS_HEADER_CONTENT_TYPE};

#[derive(serde::Serialize)]
struct TextReply<'a> {
    text: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerReply {
    content_type: Option<String>,
    body: Bytes,
}

impl ConsumerReply {
    pub fn from_message(message: &Message) -> Option<Self> {
        if message.payload.is_empty() {
            return None;
        }
        let content_type = message
            .headers
            .as_ref()
            .and_then(|headers| headers.get(NATS_HEADER_CONTENT_TYPE))
            .map(|value| value.as_str().to_owned());
        Some(Self {
            content_type,
            body: message.payload.clone(),
        })
    }

    fn is_text(&self) -> bool {
        self.content_type
            .as_deref()
            .and_then(|content_type| content_type.split(';').next())
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case(CONTENT_TYPE_TEXT))
    }

    pub fn socket_payload(&self) -> Option<Box<RawValue>> {
        if self.is_text() {
            let text = std::str::from_utf8(&self.body).ok()?;
            let wrapped = serde_json::to_string(&TextReply { text }).ok()?;
            return RawValue::from_string(wrapped).ok();
        }
        serde_json::from_slice::<Box<RawValue>>(&self.body).ok()
    }

    fn into_http(self) -> Response {
        let mut response = (StatusCode::OK, self.body).into_response();
        response.headers_mut().remove(CONTENT_TYPE);
        let content_type = self
            .content_type
            .and_then(|value| HeaderValue::from_str(&value).ok())
            .unwrap_or_else(|| HeaderValue::from_static(CONTENT_TYPE_JSON));
        response.headers_mut().insert(CONTENT_TYPE, content_type);
        response
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acknowledgement {
    Status(StatusCode),
    Reply(ConsumerReply),
}

impl From<StatusCode> for Acknowledgement {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}

impl IntoResponse for Acknowledgement {
    fn into_response(self) -> Response {
        match self {
            Self::Status(status) => (status, String::new()).into_response(),
            Self::Reply(reply) => reply.into_http(),
        }
    }
}

#[cfg(test)]
mod tests;
