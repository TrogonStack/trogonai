//! Idempotent create-or-open provisioning for JetStream streams and buckets.
//!
//! [`ensure_stream`] and [`ensure_bucket`] give every read-side primitive in
//! this crate the same "create it if it is missing, otherwise validate it
//! matches what the caller needs" entry point. Both functions check for the
//! resource first with a plain get, because JetStream's `STREAM.CREATE` API
//! does not reliably reject a create issued against an already-existing
//! stream: some divergent fields (retention) trigger a `STREAM_NAME_EXIST`
//! conflict, but others (subjects, `allow_atomic_publish`) are silently
//! accepted and applied as an in-place update of the existing stream.
//! Calling create first would therefore risk mutating a stream out from
//! under a caller who only meant to validate it. The get-first check closes
//! that hole; the residual race (two callers creating the same resource for
//! the first time concurrently) is still handled by falling back to a get on
//! an already-exists conflict from create. When the resource already
//! exists, its configuration is validated against the caller's requirements
//! and a [`StreamConfigMismatchError`] or [`KvConfigMismatchError`] names the specific
//! field that diverged, rather than dumping the whole configuration.

use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::context::{CreateKeyValueError, CreateStreamError, GetStreamError, KeyValueError};
use async_nats::jetstream::kv;
use async_nats::jetstream::stream::RetentionPolicy;
use trogon_nats::jetstream::{
    is_create_key_value_already_exists, is_create_stream_already_exists, is_get_key_value_not_found,
    is_get_stream_not_found,
};

/// A single divergent field between a required and an existing stream
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StreamConfigMismatchError {
    /// The existing stream's retention policy does not match the requirement.
    #[error("stream '{stream}' retention mismatch: expected {expected:?}, found {actual:?}")]
    Retention {
        /// Name of the stream being validated.
        stream: String,
        /// Retention policy required by the caller.
        expected: RetentionPolicy,
        /// Retention policy found on the existing stream.
        actual: RetentionPolicy,
    },
    /// The existing stream's atomic publish setting does not match the requirement.
    ///
    /// A NATS server older than 2.11 does not persist `allow_atomic_publish`
    /// at all, so an existing stream on such a server always reports `false`
    /// here regardless of what was requested at creation.
    #[error("stream '{stream}' allow_atomic_publish mismatch: expected {expected}, found {actual}")]
    AllowAtomicPublish {
        /// Name of the stream being validated.
        stream: String,
        /// Atomic publish setting required by the caller.
        expected: bool,
        /// Atomic publish setting found on the existing stream.
        actual: bool,
    },
    /// The existing stream does not carry a subject the caller requires.
    #[error("stream '{stream}' is missing required subject '{subject}'")]
    MissingSubject {
        /// Name of the stream being validated.
        stream: String,
        /// Subject the caller requires but did not find.
        subject: String,
    },
}

/// A single divergent field between a required and an existing Key/Value
/// bucket configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KvConfigMismatchError {
    /// The existing bucket's history depth does not match the requirement.
    #[error("bucket '{bucket}' history mismatch: expected {expected}, found {actual}")]
    History {
        /// Name of the bucket being validated.
        bucket: String,
        /// History depth required by the caller.
        expected: i64,
        /// History depth found on the existing bucket.
        actual: i64,
    },
}

/// A validated JetStream duplicate-detection window.
///
/// This bounds how long the JetStream server retains its per-subject
/// `Nats-Msg-Id` dedup table for duplicate detection, an operational
/// capacity knob on stream provisioning rather than a delivery guarantee.
/// NATS itself accepts a zero window (meaning "use the server's own
/// default"), but a caller-configured window of zero silently drops that
/// intent, so this type rejects it: callers who want the server default
/// should not call [`apply_duplicate_window`] at all. A non-zero window
/// below 100ms is rejected because the NATS server itself rejects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DuplicateWindow(Duration);

impl DuplicateWindow {
    /// The smallest non-zero duplicate window the NATS server accepts.
    pub const MINIMUM: Duration = Duration::from_millis(100);

    /// Creates a duplicate window after rejecting zero and any non-zero
    /// value below [`DuplicateWindow::MINIMUM`].
    pub const fn try_new(value: Duration) -> Result<Self, InvalidDuplicateWindowError> {
        if value.is_zero() {
            return Err(InvalidDuplicateWindowError::Zero);
        }
        if value.as_nanos() < Self::MINIMUM.as_nanos() {
            return Err(InvalidDuplicateWindowError::BelowMinimum {
                value,
                minimum: Self::MINIMUM,
            });
        }
        Ok(Self(value))
    }

    /// Returns the window as a plain [`Duration`] for adapter APIs.
    pub const fn as_duration(self) -> Duration {
        self.0
    }
}

impl TryFrom<Duration> for DuplicateWindow {
    type Error = InvalidDuplicateWindowError;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<DuplicateWindow> for Duration {
    fn from(value: DuplicateWindow) -> Self {
        value.as_duration()
    }
}

/// Error returned when constructing an invalid [`DuplicateWindow`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidDuplicateWindowError {
    /// The caller supplied a zero duplicate window.
    #[error("duplicate window must be non-zero")]
    Zero,
    /// The caller supplied a non-zero duplicate window below what the NATS
    /// server accepts.
    #[error("duplicate window must be at least {minimum:?}, got {value:?}")]
    BelowMinimum {
        /// The rejected duplicate window.
        value: Duration,
        /// The smallest duplicate window the NATS server accepts.
        minimum: Duration,
    },
}

/// Error returned when a [`DuplicateWindow`] is wider than a stream's `max_age`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("duplicate window {duplicate_window:?} exceeds stream max age {max_age:?}")]
pub struct DuplicateWindowExceedsMaxAgeError {
    /// The duplicate window that was rejected.
    pub duplicate_window: Duration,
    /// The stream's configured `max_age`.
    pub max_age: Duration,
}

/// Applies a validated duplicate detection window to a stream configuration.
///
/// JetStream rejects a `duplicate_window` wider than a non-zero `max_age`;
/// this is checked here rather than left for the server to reject at create
/// time, so the failure is typed and attributable to the specific field
/// instead of an opaque create error.
pub fn apply_duplicate_window(
    mut config: jetstream::stream::Config,
    duplicate_window: DuplicateWindow,
) -> Result<jetstream::stream::Config, DuplicateWindowExceedsMaxAgeError> {
    let duplicate_window = duplicate_window.as_duration();
    if !config.max_age.is_zero() && duplicate_window.as_nanos() > config.max_age.as_nanos() {
        return Err(DuplicateWindowExceedsMaxAgeError {
            duplicate_window,
            max_age: config.max_age,
        });
    }
    config.duplicate_window = duplicate_window;
    Ok(config)
}

/// Error returned by [`ensure_stream`].
#[derive(Debug, thiserror::Error)]
pub enum EnsureStreamError {
    /// Creating the stream failed for a reason other than the stream already existing.
    #[error("failed to create stream '{name}'")]
    Create {
        /// Name of the stream that failed to create.
        name: String,
        /// Underlying `async_nats` error.
        #[source]
        source: CreateStreamError,
    },
    /// The stream already existed but fetching its current configuration failed.
    #[error("failed to get existing stream '{name}'")]
    Get {
        /// Name of the stream that failed to fetch.
        name: String,
        /// Underlying `async_nats` error.
        #[source]
        source: GetStreamError,
    },
    /// The stream already existed with a configuration that diverges from the requirement.
    #[error(transparent)]
    ConfigMismatch(#[from] StreamConfigMismatchError),
}

/// Error returned by [`ensure_bucket`].
#[derive(Debug, thiserror::Error)]
pub enum EnsureBucketError {
    /// Creating the bucket failed for a reason other than the bucket already existing.
    #[error("failed to create bucket '{bucket}'")]
    Create {
        /// Name of the bucket that failed to create.
        bucket: String,
        /// Underlying `async_nats` error.
        #[source]
        source: CreateKeyValueError,
    },
    /// The bucket already existed but fetching its current configuration failed.
    #[error("failed to get existing bucket '{bucket}'")]
    Get {
        /// Name of the bucket that failed to fetch.
        bucket: String,
        /// Underlying `async_nats` error.
        #[source]
        source: KeyValueError,
    },
    /// The bucket already existed with a configuration that diverges from the requirement.
    #[error(transparent)]
    ConfigMismatch(#[from] KvConfigMismatchError),
}

/// Creates a JetStream stream, or opens it if it already exists.
///
/// `config` is both the configuration used to create the stream and the
/// requirement checked against an already-existing stream. Validation covers
/// [`jetstream::stream::Config::retention`], [`jetstream::stream::Config::allow_atomic_publish`],
/// and subject coverage (every subject in `config.subjects` must already be
/// present on the existing stream); it does not compare every field, so
/// changes to unvalidated fields (limits, storage type, replicas, ...) on an
/// existing stream go unnoticed. Concurrent callers racing to create the same
/// stream both succeed: the loser's create fails with an already-exists
/// error, which this function treats as "someone else already created it"
/// and resolves with a get instead of surfacing an error.
pub async fn ensure_stream(
    js: &jetstream::Context,
    config: jetstream::stream::Config,
) -> Result<jetstream::stream::Stream, EnsureStreamError> {
    let name = config.name.clone();

    match js.get_stream(&name).await {
        Ok(stream) => {
            validate_stream_config(&name, &config, stream.cached_info())?;
            return Ok(stream);
        }
        Err(source) if is_get_stream_not_found(&source) => {}
        Err(source) => return Err(EnsureStreamError::Get { name, source }),
    }

    match js.create_stream(config.clone()).await {
        Ok(stream) => Ok(stream),
        Err(source) if is_create_stream_already_exists(&source) => {
            let stream = js.get_stream(&name).await.map_err(|source| EnsureStreamError::Get {
                name: name.clone(),
                source,
            })?;
            validate_stream_config(&name, &config, stream.cached_info())?;
            Ok(stream)
        }
        Err(source) => Err(EnsureStreamError::Create { name, source }),
    }
}

/// Creates a Key/Value bucket, or opens it if it already exists.
///
/// `config` is both the configuration used to create the bucket and the
/// requirement checked against an already-existing bucket. Validation covers
/// [`kv::Config::history`] only; it does not compare every field. Concurrent
/// callers racing to create the same bucket both succeed, following the same
/// get-first-then-create-with-fallback sequencing as [`ensure_stream`].
pub async fn ensure_bucket(js: &jetstream::Context, config: kv::Config) -> Result<kv::Store, EnsureBucketError> {
    let bucket = config.bucket.clone();

    match js.get_key_value(&bucket).await {
        Ok(store) => {
            validate_kv_config(&bucket, &config, &store.stream.cached_info().config)?;
            return Ok(store);
        }
        Err(source) if is_get_key_value_not_found(&source) => {}
        Err(source) => return Err(EnsureBucketError::Get { bucket, source }),
    }

    match js.create_key_value(config.clone()).await {
        Ok(store) => Ok(store),
        Err(source) if is_create_key_value_already_exists(&source) => {
            let store = js
                .get_key_value(&bucket)
                .await
                .map_err(|source| EnsureBucketError::Get {
                    bucket: bucket.clone(),
                    source,
                })?;
            validate_kv_config(&bucket, &config, &store.stream.cached_info().config)?;
            Ok(store)
        }
        Err(source) => Err(EnsureBucketError::Create { bucket, source }),
    }
}

fn validate_stream_config(
    name: &str,
    required: &jetstream::stream::Config,
    actual: &jetstream::stream::Info,
) -> Result<(), StreamConfigMismatchError> {
    let actual = &actual.config;
    if actual.retention != required.retention {
        return Err(StreamConfigMismatchError::Retention {
            stream: name.to_string(),
            expected: required.retention,
            actual: actual.retention,
        });
    }
    if actual.allow_atomic_publish != required.allow_atomic_publish {
        return Err(StreamConfigMismatchError::AllowAtomicPublish {
            stream: name.to_string(),
            expected: required.allow_atomic_publish,
            actual: actual.allow_atomic_publish,
        });
    }
    for subject in &required.subjects {
        if !actual.subjects.contains(subject) {
            return Err(StreamConfigMismatchError::MissingSubject {
                stream: name.to_string(),
                subject: subject.clone(),
            });
        }
    }
    Ok(())
}

fn validate_kv_config(
    bucket: &str,
    required: &kv::Config,
    actual: &jetstream::stream::Config,
) -> Result<(), KvConfigMismatchError> {
    if actual.max_messages_per_subject != required.history {
        return Err(KvConfigMismatchError::History {
            bucket: bucket.to_string(),
            expected: required.history,
            actual: actual.max_messages_per_subject,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
