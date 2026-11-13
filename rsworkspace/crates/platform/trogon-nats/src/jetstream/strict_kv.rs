use async_nats::{
    HeaderMap,
    jetstream::{
        kv,
        stream::{LastRawMessageError, LastRawMessageErrorKind},
    },
};
use std::{num::NonZeroUsize, time::Duration};

/// Certifies duplicate-preserving native read metadata, not mutation semantics.
pub trait JetStreamStrictKvRead: super::JetStreamKvGet + super::JetStreamKvEntry {}

#[derive(Clone)]
pub struct StrictNativeKvStore {
    context: async_nats::jetstream::Context,
    bucket: kv::Store,
    maximum_record_bytes: NonZeroUsize,
    read_timeout: Duration,
}
impl StrictNativeKvStore {
    pub fn new(
        context: async_nats::jetstream::Context,
        bucket: kv::Store,
        maximum_record_bytes: NonZeroUsize,
        read_timeout: Duration,
    ) -> Result<Self, NativeKvReadError> {
        let scope = super::NativeKvScope::new(
            crate::NatsToken::new(&bucket.name).map_err(|_| NativeKvReadError::InvalidBinding)?,
        )
        .map_err(|_| NativeKvReadError::InvalidBinding)?;
        if read_timeout.is_zero()
            || read_timeout > Duration::from_secs(30)
            || maximum_record_bytes.get() > 64 * 1024 * 1024
            || bucket.stream_name != scope.stream()
            || bucket.stream.cached_info().config.name != scope.stream()
            || bucket.prefix != format!("$KV.{}.", bucket.name)
            || bucket.put_prefix.is_some()
            || bucket.use_jetstream_prefix
        {
            return Err(NativeKvReadError::InvalidBinding);
        }
        Ok(Self {
            context,
            bucket,
            maximum_record_bytes,
            read_timeout,
        })
    }
    pub fn bucket(&self) -> &kv::Store {
        &self.bucket
    }

    async fn read(&self, key: &str) -> Result<Option<kv::Entry>, NativeKvReadError> {
        use async_nats::jetstream::{ErrorCode, response::Response, stream::RawMessage};
        use futures::StreamExt;
        let key = super::NativeKvKey::new(key).map_err(|_| NativeKvReadError::InvalidKey)?;
        let subject = format!("{}{}", self.bucket.prefix, key.as_str());
        let request = serde_json::to_vec(&serde_json::json!({"last_by_subj":subject}))
            .map_err(|_| NativeKvReadError::InvalidRecord)?;
        #[derive(serde::Deserialize)]
        struct RawResponse {
            message: RawMessage,
        }
        let response = tokio::time::timeout(self.read_timeout, async {
            let client = self.context.client();
            let inbox = client.new_inbox();
            let mut subscription = client
                .subscribe(inbox.clone())
                .await
                .map_err(NativeKvReadError::Subscribe)?;
            self.context
                .send_request(
                    format!("STREAM.MSG.GET.{}", self.bucket.stream_name),
                    async_nats::Request::new().inbox(inbox).payload(request.into()),
                )
                .await
                .map_err(NativeKvReadError::Send)?;
            let message = subscription.next().await.ok_or(NativeKvReadError::MissingResponse)?;
            if message.status.is_some() {
                return Err(NativeKvReadError::InvalidRecord);
            }
            let maximum_wire_bytes = (self.maximum_record_bytes.get() + MAXIMUM_RAW_HEADER_BYTES) * 2 + 4096;
            if message.payload.len() > maximum_wire_bytes {
                return Err(NativeKvReadError::RecordLimit);
            }
            serde_json::from_slice::<Response<RawResponse>>(&message.payload)
                .map_err(|_| NativeKvReadError::InvalidRecord)
        })
        .await
        .map_err(|_| NativeKvReadError::Timeout)??;
        let raw = match response {
            Response::Err { error } if error.error_code() == ErrorCode::NO_MESSAGE_FOUND => return Ok(None),
            Response::Err { error } => return Err(NativeKvReadError::Api(error)),
            Response::Ok(response) => response.message,
        };
        if raw.subject != subject || raw.sequence == 0 {
            return Err(NativeKvReadError::InvalidRecord);
        }
        let operation = match raw.headers.as_deref() {
            None => kv::Operation::Put,
            Some(encoded) => {
                let bytes = decode_bounded(encoded, MAXIMUM_RAW_HEADER_BYTES)?;
                strict_native_kv_raw_operation(&bytes)?
            }
        };
        let value = decode_bounded(&raw.payload, self.maximum_record_bytes.get())?;
        Ok(Some(kv::Entry {
            bucket: self.bucket.name.clone(),
            key: key.as_str().into(),
            value: value.into(),
            revision: raw.sequence,
            created: raw.time,
            operation,
            delta: 0,
            seen_current: false,
        }))
    }
}
impl JetStreamStrictKvRead for StrictNativeKvStore {}
impl super::JetStreamKvEntry for StrictNativeKvStore {
    async fn entry(&self, key: String) -> Result<Option<kv::Entry>, kv::EntryError> {
        self.read(&key).await.map_err(Into::into)
    }
}
impl super::JetStreamKvGet for StrictNativeKvStore {
    async fn get(&self, key: String) -> Result<Option<bytes::Bytes>, kv::EntryError> {
        Ok(self
            .read(&key)
            .await
            .map_err(kv::EntryError::from)?
            .and_then(|entry| (entry.operation == kv::Operation::Put).then_some(entry.value)))
    }
}
impl super::JetStreamKeyValueStatus for StrictNativeKvStore {
    async fn status(&self) -> Result<kv::bucket::Status, kv::StatusError> {
        self.bucket.status().await
    }
}
impl super::JetStreamKeyValueUpdate for StrictNativeKvStore {
    async fn update(&self, key: &str, value: bytes::Bytes, revision: u64) -> Result<u64, kv::UpdateError> {
        self.bucket.update(key, value, revision).await
    }
}
impl super::JetStreamConditionalKvUpdate for StrictNativeKvStore {
    async fn conditional_update<P>(
        &self,
        publisher: &P,
        request: super::NativeKvUpdate<'_>,
    ) -> Result<std::num::NonZeroU64, super::NativeKvCasError>
    where
        P: super::JetStreamPublishMessage<PublishError = async_nats::jetstream::context::PublishError>,
    {
        self.bucket.conditional_update(publisher, request).await
    }
}

const MAXIMUM_RAW_HEADER_BYTES: usize = 16 * 1024;
fn decode_bounded(value: &str, maximum: usize) -> Result<Vec<u8>, NativeKvReadError> {
    use base64::Engine;
    if value.len() > maximum.div_ceil(3) * 4 {
        return Err(NativeKvReadError::RecordLimit);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| NativeKvReadError::InvalidRecord)?;
    if decoded.len() > maximum {
        return Err(NativeKvReadError::RecordLimit);
    }
    Ok(decoded)
}

/// Parses original stored header bytes before SDK conversion can fold duplicates.
pub fn strict_native_kv_raw_operation(raw: &[u8]) -> Result<kv::Operation, NativeKvReadError> {
    let headers = super::parse_native_stored_headers(raw).map_err(|error| match error {
        super::NativeStreamReadError::Limit => NativeKvReadError::RecordLimit,
        _ => NativeKvReadError::InvalidRecord,
    })?;
    strict_native_kv_operation(Some(&headers))
}

#[derive(thiserror::Error)]
pub enum NativeKvReadError {
    #[error("native KV read binding is invalid")]
    InvalidBinding,
    #[error("native KV read key is invalid")]
    InvalidKey,
    #[error("native KV record identity is invalid")]
    InvalidRecord,
    #[error("native KV operation header is ambiguous")]
    AmbiguousOperation,
    #[error("native KV operation is unsupported")]
    UnknownOperation,
    #[error("native KV marker is unsupported")]
    UnknownMarker,
    #[error("native KV authority header is unsupported")]
    UnsupportedHeader,
    #[error("native KV record exceeds its bound")]
    RecordLimit,
    #[error("native KV read deadline elapsed")]
    Timeout,
    #[error("native KV response is missing")]
    MissingResponse,
    #[error("native KV read inbox subscription failed")]
    Subscribe(#[source] async_nats::SubscribeError),
    #[error("native KV leader request failed")]
    Send(#[source] async_nats::PublishError),
    #[error("native KV leader API refused the request")]
    Api(#[source] async_nats::jetstream::Error),
    #[error("native KV leader read failed")]
    Read(#[source] LastRawMessageError),
}

pub fn strict_native_kv_operation(headers: Option<&HeaderMap>) -> Result<kv::Operation, NativeKvReadError> {
    let Some(headers) = headers else {
        return Ok(kv::Operation::Put);
    };
    let mut operation = None;
    let mut marker = None;
    for (name, values) in headers.iter() {
        let name = AsRef::<str>::as_ref(name);
        if name.eq_ignore_ascii_case("KV-Operation") {
            if name != "KV-Operation" || values.len() != 1 || operation.is_some() {
                return Err(NativeKvReadError::AmbiguousOperation);
            }
            operation = values.first().map(|value| value.as_str());
        } else if name.eq_ignore_ascii_case("Nats-Marker-Reason") {
            if name != "Nats-Marker-Reason" || values.len() != 1 || marker.is_some() {
                return Err(NativeKvReadError::AmbiguousOperation);
            }
            marker = values.first().map(|value| value.as_str());
        } else if name.get(..3).is_some_and(|prefix| prefix.eq_ignore_ascii_case("KV-"))
            || name
                .get(..12)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Nats-Marker-"))
        {
            return Err(NativeKvReadError::UnsupportedHeader);
        }
    }
    match (operation, marker) {
        (Some(_), Some(_)) => Err(NativeKvReadError::AmbiguousOperation),
        (Some("PUT"), None) | (None, None) => Ok(kv::Operation::Put),
        (Some("DEL"), None) | (None, Some("Remove")) => Ok(kv::Operation::Delete),
        (Some("PURGE"), None) | (None, Some("MaxAge" | "Purge")) => Ok(kv::Operation::Purge),
        (Some(_), None) => Err(NativeKvReadError::UnknownOperation),
        (None, Some(_)) => Err(NativeKvReadError::UnknownMarker),
    }
}

/// Legacy leader read rejects unknown metadata but SDK conversion loses duplicates.
/// Authority consumers require StrictNativeKvStore instead. An absent response
/// is data only; it never grants creation, restoration or pruning authority.
pub async fn strict_native_kv_entry(bucket: &kv::Store, key: &str) -> Result<Option<kv::Entry>, NativeKvReadError> {
    if key.is_empty()
        || key.starts_with('.')
        || key.ends_with('.')
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_/=.".contains(&byte))
    {
        return Err(NativeKvReadError::InvalidKey);
    }
    if bucket.stream_name != bucket.stream.cached_info().config.name
        || !bucket.prefix.starts_with("$KV.")
        || !bucket.prefix.ends_with('.')
    {
        return Err(NativeKvReadError::InvalidBinding);
    }
    let subject = format!("{}{}", bucket.prefix, key);
    let message = match bucket.stream.get_last_raw_message_by_subject(&subject).await {
        Ok(message) => message,
        Err(error) if error.kind() == LastRawMessageErrorKind::NoMessageFound => return Ok(None),
        Err(error) => return Err(NativeKvReadError::Read(error)),
    };
    if message.subject.as_str() != subject || message.sequence == 0 {
        return Err(NativeKvReadError::InvalidRecord);
    }
    let operation = strict_native_kv_operation(Some(&message.headers))?;
    Ok(Some(kv::Entry {
        bucket: bucket.name.clone(),
        key: key.into(),
        value: message.payload,
        revision: message.sequence,
        created: message.time,
        operation,
        delta: 0,
        seen_current: false,
    }))
}

impl From<NativeKvReadError> for kv::EntryError {
    fn from(source: NativeKvReadError) -> Self {
        let kind = match &source {
            NativeKvReadError::InvalidKey => kv::EntryErrorKind::InvalidKey,
            NativeKvReadError::Read(error) if error.kind() == LastRawMessageErrorKind::InvalidSubject => {
                kv::EntryErrorKind::InvalidKey
            }
            _ => kv::EntryErrorKind::Other,
        };
        Self::with_source(kind, source)
    }
}
impl std::fmt::Debug for NativeKvReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(_) | Self::Subscribe(_) | Self::Send(_) | Self::Api(_) => {
                formatter.debug_struct("NativeKvReadError").finish_non_exhaustive()
            }
            _ => write!(formatter, "{self}"),
        }
    }
}

#[cfg(test)]
mod tests;
