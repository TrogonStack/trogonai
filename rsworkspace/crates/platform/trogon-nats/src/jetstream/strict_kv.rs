use async_nats::{HeaderMap, jetstream::kv};
use std::{future::Future, num::NonZeroUsize, time::Duration};

/// Duplicate-preserving native reads retain typed record and transport failures.
pub trait JetStreamStrictKvRead {
    fn read_entry(&self, key: &str) -> impl Future<Output = Result<Option<kv::Entry>, NativeKvReadError>> + Send;
}

#[derive(Clone)]
pub struct StrictNativeKvStore {
    reader: super::StrictNativeStreamReader,
    bucket: kv::Store,
    scope: super::NativeKvScope,
}
impl StrictNativeKvStore {
    pub async fn new(
        context: async_nats::jetstream::Context,
        scope: super::NativeKvScope,
        maximum_record_bytes: NonZeroUsize,
        read_timeout: Duration,
    ) -> Result<Self, NativeKvReadError> {
        let reader = super::StrictNativeStreamReader::new(context.clone(), maximum_record_bytes, read_timeout)
            .map_err(NativeKvReadError::Read)?;
        let bucket = tokio::time::timeout(read_timeout, context.get_key_value(scope.bucket().as_str()))
            .await
            .map_err(NativeKvReadError::Timeout)?
            .map_err(NativeKvReadError::Lookup)?;
        if bucket.name != scope.bucket().as_str()
            || bucket.stream_name != scope.stream()
            || bucket.stream.cached_info().config.name != scope.stream()
            || bucket.prefix != format!("$KV.{}.", bucket.name)
            || bucket.put_prefix.is_some()
            || bucket.use_jetstream_prefix
        {
            return Err(NativeKvReadError::InvalidBinding);
        }
        Ok(Self { reader, bucket, scope })
    }
    pub fn bucket(&self) -> &kv::Store {
        &self.bucket
    }
    pub async fn read_entry(&self, key: &str) -> Result<Option<kv::Entry>, NativeKvReadError> {
        let key = super::NativeKvKey::new(key).map_err(|_| NativeKvReadError::InvalidKey)?;
        let Some(message) = self
            .reader
            .read_kv_last(&self.scope, &key)
            .await
            .map_err(NativeKvReadError::Read)?
        else {
            return Ok(None);
        };
        let operation = strict_native_kv_operation(Some(&message.headers))?;
        Ok(Some(kv::Entry {
            bucket: self.bucket.name.clone(),
            key: key.as_str().into(),
            value: message.payload,
            revision: message.sequence,
            created: message.time,
            operation,
            delta: 0,
            seen_current: false,
        }))
    }
}
impl JetStreamStrictKvRead for StrictNativeKvStore {
    async fn read_entry(&self, key: &str) -> Result<Option<kv::Entry>, NativeKvReadError> {
        self.read_entry(key).await
    }
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
    #[error("native KV binding lookup deadline elapsed")]
    Timeout(#[source] tokio::time::error::Elapsed),
    #[error("native KV binding lookup failed")]
    Lookup(#[source] async_nats::jetstream::context::KeyValueError),
    #[error("native KV leader read failed")]
    Read(#[source] super::NativeStreamReadError),
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

impl std::fmt::Debug for NativeKvReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(_) | Self::Lookup(_) => formatter.debug_struct("NativeKvReadError").finish_non_exhaustive(),
            _ => write!(formatter, "{self}"),
        }
    }
}

#[cfg(test)]
mod tests;
