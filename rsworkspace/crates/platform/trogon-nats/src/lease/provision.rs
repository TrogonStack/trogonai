use std::time::Duration;

use async_nats::jetstream::{
    context::{CreateKeyValueError, KeyValueError},
    kv,
};

use crate::jetstream::is_create_key_value_already_exists;
use crate::jetstream::{JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamKeyValueStatus};

use super::{IncompatibleLeaseBucketConfig, LeaseError, LeaseProvisionError, NatsKvLease, NatsKvLeaseConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(coverage, allow(dead_code))]
pub(super) struct KeyValueSettings {
    pub history: i64,
    pub max_age: Duration,
    pub allow_message_ttl: bool,
    pub subject_delete_marker_ttl: Option<Duration>,
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) fn create_bucket_error(source: CreateKeyValueError) -> LeaseError {
    LeaseError::provision_source(
        "failed to create lease bucket",
        LeaseProvisionError::CreateBucket(source),
    )
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) fn open_existing_bucket_error(source: KeyValueError) -> LeaseError {
    LeaseError::provision_source(
        "failed to open existing lease bucket after create reported already exists",
        LeaseProvisionError::OpenExistingBucket(source),
    )
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) fn inspect_bucket_error(source: kv::StatusError) -> LeaseError {
    LeaseError::provision_source(
        "failed to inspect lease bucket configuration",
        LeaseProvisionError::InspectBucket(source),
    )
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) fn key_value_config(config: &NatsKvLeaseConfig) -> kv::Config {
    kv::Config {
        bucket: config.bucket().as_str().to_owned(),
        history: 1,
        max_age: Duration::ZERO,
        limit_markers: Some(config.ttl()),
        ..Default::default()
    }
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) async fn provision_store<J, S>(js: &J, config: &NatsKvLeaseConfig) -> Result<S, LeaseError>
where
    J: JetStreamCreateKeyValue<Store = S> + JetStreamGetKeyValue<Store = S>,
    S: JetStreamKeyValueStatus,
{
    let store = match js.create_key_value(key_value_config(config)).await {
        Ok(store) => store,
        Err(source) if is_create_key_value_already_exists(&source) => js
            .get_key_value(config.bucket().as_str().to_owned())
            .await
            .map_err(open_existing_bucket_error)?,
        Err(source) => {
            return Err(create_bucket_error(source));
        }
    };

    let status = store.status().await.map_err(inspect_bucket_error)?;
    let settings = settings_from_status(&status);
    validate_bucket_settings(settings, config)?;

    Ok(store)
}

impl NatsKvLease {
    pub async fn provision(
        js: &async_nats::jetstream::Context,
        config: &NatsKvLeaseConfig,
    ) -> Result<Self, LeaseError> {
        let store = provision_store(js, config).await?;
        let mut lease = Self::new(store, config.key().clone());
        lease.ttl = Some(config.ttl());
        lease.js_context = Some(js.clone());

        Ok(lease)
    }
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) fn validate_bucket_settings(
    settings: KeyValueSettings,
    config: &NatsKvLeaseConfig,
) -> Result<(), LeaseError> {
    let expected = KeyValueSettings {
        history: 1,
        max_age: Duration::ZERO,
        allow_message_ttl: true,
        subject_delete_marker_ttl: Some(config.ttl()),
    };

    if settings != expected {
        return Err(LeaseError::IncompatibleBucketConfig {
            source: IncompatibleLeaseBucketConfig {
                expected_history: expected.history,
                actual_history: settings.history,
                expected_max_age: expected.max_age,
                actual_max_age: settings.max_age,
                expected_allow_message_ttl: expected.allow_message_ttl,
                actual_allow_message_ttl: settings.allow_message_ttl,
                expected_subject_delete_marker_ttl: expected.subject_delete_marker_ttl,
                actual_subject_delete_marker_ttl: settings.subject_delete_marker_ttl,
            },
        });
    }

    Ok(())
}

#[cfg_attr(coverage, allow(dead_code))]
pub(super) fn settings_from_status(status: &async_nats::jetstream::kv::bucket::Status) -> KeyValueSettings {
    KeyValueSettings {
        history: status.history(),
        max_age: status.max_age(),
        allow_message_ttl: status.info.config.allow_message_ttl,
        subject_delete_marker_ttl: status.info.config.subject_delete_marker_ttl,
    }
}

#[cfg(test)]
mod tests;
