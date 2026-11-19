use super::{native_stream_get_request::NativeStreamGetRequest, native_stream_get_response::NativeStreamGetResponse};
use crate::{
    NatsToken,
    constants::{MAX_NATIVE_STORED_HEADER_BYTES, MAX_SUBJECT_BYTES},
};
use async_nats::{
    HeaderMap,
    jetstream::{Context, message::StreamMessage},
};
use base64::Engine;
use futures::StreamExt;
use std::{
    num::{NonZeroU64, NonZeroUsize},
    str::FromStr,
    time::Duration,
};

/// Bounded leader observations with original header cardinality. Neither an
/// empty result nor a successful read proves quorum-complete absence.
#[derive(Clone)]
pub struct StrictNativeStreamReader {
    context: Context,
    maximum_record_bytes: NonZeroUsize,
    timeout: Duration,
}
#[derive(Debug, thiserror::Error)]
pub enum NativeStreamReadError {
    #[error("native stream read binding is invalid")]
    InvalidBinding,
    #[error("native stream record exceeds its bounded profile")]
    Limit,
    #[error("native stream record is malformed")]
    InvalidRecord,
    #[error("native stream reply subscription failed")]
    Subscribe(#[source] async_nats::SubscribeError),
    #[error("native stream request publication failed")]
    Send(#[source] async_nats::PublishError),
    #[error("native stream reply subscription closed")]
    Closed,
    #[error("native stream request received protocol status {0}")]
    ProtocolStatus(async_nats::StatusCode),
    #[error("native stream header name is malformed")]
    HeaderName(#[source] async_nats::header::ParseHeaderNameError),
    #[error("native stream header value is malformed")]
    HeaderValue(#[source] async_nats::header::ParseHeaderValueError),
    #[error("native stream read deadline expired")]
    Timeout(#[source] tokio::time::error::Elapsed),
    #[error("native stream API refused the read")]
    Api(#[source] async_nats::jetstream::Error),
    #[error("native stream API response is malformed")]
    Json(#[source] serde_json::Error),
}
impl StrictNativeStreamReader {
    pub fn new(
        context: Context,
        maximum_record_bytes: NonZeroUsize,
        timeout: Duration,
    ) -> Result<Self, NativeStreamReadError> {
        if maximum_record_bytes.get() > 64 * 1024 * 1024 || timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(NativeStreamReadError::InvalidBinding);
        }
        Ok(Self {
            context,
            maximum_record_bytes,
            timeout,
        })
    }
    pub async fn read_kv_sequence(
        &self,
        scope: &super::NativeKvScope,
        key: &super::NativeKvKey,
        sequence: NonZeroU64,
    ) -> Result<Option<StreamMessage>, NativeStreamReadError> {
        let stream = scope.stream_token();
        let subject = scope.subject(key);
        self.read(
            stream,
            NativeStreamGetRequest::Sequence { seq: sequence.get() },
            Some(&subject),
            Some(sequence.get()),
        )
        .await
    }
    /// Native KV keys have their own validated grammar, separate from domain event subjects.
    pub async fn read_kv_last(
        &self,
        scope: &super::NativeKvScope,
        key: &super::NativeKvKey,
    ) -> Result<Option<StreamMessage>, NativeStreamReadError> {
        let stream = scope.stream_token();
        let subject = scope.subject(key);
        self.read(
            stream,
            NativeStreamGetRequest::LastBySubject {
                last_by_subj: subject.as_str(),
            },
            Some(&subject),
            None,
        )
        .await
    }
    pub async fn read_sequence(
        &self,
        stream: &NatsToken,
        sequence: NonZeroU64,
    ) -> Result<Option<StreamMessage>, NativeStreamReadError> {
        self.read(
            stream,
            NativeStreamGetRequest::Sequence { seq: sequence.get() },
            None,
            Some(sequence.get()),
        )
        .await
    }
    pub async fn read_first(
        &self,
        stream: &NatsToken,
        subject: &str,
    ) -> Result<Option<StreamMessage>, NativeStreamReadError> {
        validate_subject(subject)?;
        self.read(
            stream,
            NativeStreamGetRequest::NextBySubject {
                seq: 0,
                next_by_subj: subject,
            },
            Some(subject),
            None,
        )
        .await
    }
    /// Reads the next stored record of this exact subject at or after sequence.
    /// A missing record is an observation, not quorum-complete absence.
    pub async fn read_next(
        &self,
        stream: &NatsToken,
        subject: &str,
        sequence: NonZeroU64,
    ) -> Result<Option<StreamMessage>, NativeStreamReadError> {
        validate_subject(subject)?;
        let result = self
            .read(
                stream,
                NativeStreamGetRequest::NextBySubject {
                    seq: sequence.get(),
                    next_by_subj: subject,
                },
                Some(subject),
                None,
            )
            .await?;
        if result.as_ref().is_some_and(|record| record.sequence < sequence.get()) {
            return Err(NativeStreamReadError::InvalidRecord);
        }
        Ok(result)
    }
    pub async fn read_last(
        &self,
        stream: &NatsToken,
        subject: &str,
    ) -> Result<Option<StreamMessage>, NativeStreamReadError> {
        validate_subject(subject)?;
        self.read(
            stream,
            NativeStreamGetRequest::LastBySubject { last_by_subj: subject },
            Some(subject),
            None,
        )
        .await
    }
    async fn read(
        &self,
        stream: &NatsToken,
        request: NativeStreamGetRequest<'_>,
        subject: Option<&str>,
        sequence: Option<u64>,
    ) -> Result<Option<StreamMessage>, NativeStreamReadError> {
        if !stream
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return Err(NativeStreamReadError::InvalidBinding);
        }
        let request = serde_json::to_vec(&request).map_err(NativeStreamReadError::Json)?;
        let response = tokio::time::timeout(self.timeout, async {
            let client = self.context.client();
            let inbox = client.new_inbox();
            let mut subscription = client
                .subscribe(inbox.clone())
                .await
                .map_err(NativeStreamReadError::Subscribe)?;
            self.context
                .send_request(
                    format!("STREAM.MSG.GET.{stream}"),
                    async_nats::Request::new().inbox(inbox).payload(request.into()),
                )
                .await
                .map_err(NativeStreamReadError::Send)?;
            let message = subscription.next().await.ok_or(NativeStreamReadError::Closed)?;
            if let Some(status) = message.status {
                return Err(NativeStreamReadError::ProtocolStatus(status));
            }
            let maximum = (self.maximum_record_bytes.get() + MAX_NATIVE_STORED_HEADER_BYTES) * 2 + 4096;
            if message.payload.len() > maximum {
                return Err(NativeStreamReadError::Limit);
            }
            serde_json::from_slice::<async_nats::jetstream::response::Response<NativeStreamGetResponse>>(
                &message.payload,
            )
            .map_err(NativeStreamReadError::Json)
        })
        .await
        .map_err(NativeStreamReadError::Timeout)??;
        let raw = match response {
            async_nats::jetstream::response::Response::Err { error }
                if error.error_code() == async_nats::jetstream::ErrorCode::NO_MESSAGE_FOUND =>
            {
                return Ok(None);
            }
            async_nats::jetstream::response::Response::Err { error } => return Err(NativeStreamReadError::Api(error)),
            async_nats::jetstream::response::Response::Ok(response) => response.message,
        };
        if raw.subject.starts_with("$KV.") {
            if subject != Some(raw.subject.as_str()) {
                return Err(NativeStreamReadError::InvalidRecord);
            }
        } else {
            validate_subject(&raw.subject).map_err(|_| NativeStreamReadError::InvalidRecord)?;
        }
        if raw.sequence == 0
            || subject.is_some_and(|subject| subject != raw.subject)
            || sequence.is_some_and(|sequence| sequence != raw.sequence)
        {
            return Err(NativeStreamReadError::InvalidRecord);
        }
        let headers = match raw.headers {
            None => HeaderMap::new(),
            Some(encoded) => parse_native_stored_headers(&decode(&encoded, MAX_NATIVE_STORED_HEADER_BYTES)?)?,
        };
        let payload = decode(&raw.payload, self.maximum_record_bytes.get())?;
        Ok(Some(StreamMessage {
            subject: raw.subject.into(),
            sequence: raw.sequence,
            headers,
            payload: payload.into(),
            time: raw.time,
        }))
    }
}
fn validate_subject(subject: &str) -> Result<(), NativeStreamReadError> {
    if subject.is_empty()
        || subject.len() > MAX_SUBJECT_BYTES
        || subject.split('.').any(|token| {
            token.is_empty()
                || !token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        })
    {
        return Err(NativeStreamReadError::InvalidBinding);
    }
    Ok(())
}
fn decode(value: &str, maximum: usize) -> Result<Vec<u8>, NativeStreamReadError> {
    if value.len() > maximum.div_ceil(3) * 4 {
        return Err(NativeStreamReadError::Limit);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| NativeStreamReadError::InvalidRecord)?;
    if bytes.len() > maximum {
        return Err(NativeStreamReadError::Limit);
    }
    Ok(bytes)
}
/// Parses original stored headers without collapsing repeated values. Owners
/// still validate the cardinality and meaning of their authority metadata.
pub fn parse_native_stored_headers(raw: &[u8]) -> Result<HeaderMap, NativeStreamReadError> {
    if raw.len() > MAX_NATIVE_STORED_HEADER_BYTES {
        return Err(NativeStreamReadError::Limit);
    }
    let raw = std::str::from_utf8(raw).map_err(|_| NativeStreamReadError::InvalidRecord)?;
    let body = raw
        .strip_prefix("NATS/1.0\r\n")
        .and_then(|value| value.strip_suffix("\r\n"))
        .ok_or(NativeStreamReadError::InvalidRecord)?;
    if body.is_empty() {
        return Ok(HeaderMap::new());
    }
    let body = body.strip_suffix("\r\n").ok_or(NativeStreamReadError::InvalidRecord)?;
    let mut headers = HeaderMap::new();
    for (index, line) in body.split("\r\n").enumerate() {
        if index >= 128 || line.len() > 2048 || line.is_empty() || line.starts_with([' ', '\t']) {
            return Err(NativeStreamReadError::InvalidRecord);
        }
        let (name, value) = line.split_once(':').ok_or(NativeStreamReadError::InvalidRecord)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return Err(NativeStreamReadError::InvalidRecord);
        }
        let value = value.strip_prefix(' ').unwrap_or(value);
        if !value.bytes().all(|byte| byte == b'\t' || (32..=126).contains(&byte)) {
            return Err(NativeStreamReadError::InvalidRecord);
        }
        let name = async_nats::HeaderName::from_str(name).map_err(NativeStreamReadError::HeaderName)?;
        let value = async_nats::HeaderValue::from_str(value).map_err(NativeStreamReadError::HeaderValue)?;
        headers.append(name, value);
    }
    Ok(headers)
}
#[cfg(test)]
mod tests;
