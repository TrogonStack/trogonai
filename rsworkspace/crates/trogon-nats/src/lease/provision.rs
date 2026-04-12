use std::time::Duration;

use async_nats::jetstream::{
    context::{CreateKeyValueError, KeyValueError},
    kv,
};

use crate::jetstream::is_create_key_value_already_exists;
use crate::jetstream::{JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamKeyValueStatus};

use super::{
    IncompatibleLeaseBucketConfig, LeaseError, LeaseProvisionError, NatsKvLease, NatsKvLeaseConfig,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KeyValueSettings {
    pub history: i64,
    pub max_age: Duration,
    pub allow_message_ttl: bool,
    pub subject_delete_marker_ttl: Option<Duration>,
}

pub(super) fn create_bucket_error(source: CreateKeyValueError) -> LeaseError {
    LeaseError::provision_source(
        "failed to create lease bucket",
        LeaseProvisionError::CreateBucket(source),
    )
}

pub(super) fn open_existing_bucket_error(source: KeyValueError) -> LeaseError {
    LeaseError::provision_source(
        "failed to open existing lease bucket after create reported already exists",
        LeaseProvisionError::OpenExistingBucket(source),
    )
}

pub(super) fn inspect_bucket_error(source: kv::StatusError) -> LeaseError {
    LeaseError::provision_source(
        "failed to inspect lease bucket configuration",
        LeaseProvisionError::InspectBucket(source),
    )
}

pub(super) fn key_value_config(config: &NatsKvLeaseConfig) -> kv::Config {
    kv::Config {
        bucket: config.bucket().as_str().to_owned(),
        history: 1,
        max_age: Duration::ZERO,
        limit_markers: Some(config.ttl()),
        ..Default::default()
    }
}

pub(super) async fn provision_store<J, S>(
    js: &J,
    config: &NatsKvLeaseConfig,
) -> Result<S, LeaseError>
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

pub(super) fn settings_from_status(
    status: &async_nats::jetstream::kv::bucket::Status,
) -> KeyValueSettings {
    KeyValueSettings {
        history: status.history(),
        max_age: status.max_age(),
        allow_message_ttl: status.info.config.allow_message_ttl,
        subject_delete_marker_ttl: status.info.config.subject_delete_marker_ttl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jetstream::{MockJetStreamKvClient, MockJetStreamKvStore};
    use crate::lease::{LeaseRenewInterval, LeaseTtl};
    use async_nats::jetstream::context::CreateKeyValueErrorKind;

    fn ttl(secs: u64) -> LeaseTtl {
        LeaseTtl::from_secs(secs).unwrap()
    }

    fn renew_interval(secs: u64) -> LeaseRenewInterval {
        LeaseRenewInterval::from_secs(secs).unwrap()
    }

    fn config() -> NatsKvLeaseConfig {
        NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap()
    }

    fn strict_settings(ttl: Duration) -> KeyValueSettings {
        KeyValueSettings {
            history: 1,
            max_age: Duration::ZERO,
            allow_message_ttl: true,
            subject_delete_marker_ttl: Some(ttl),
        }
    }

    fn apply_settings(store: &MockJetStreamKvStore, settings: KeyValueSettings) {
        store.set_settings(
            settings.history,
            settings.max_age,
            settings.allow_message_ttl,
            settings.subject_delete_marker_ttl,
        );
    }

    #[test]
    fn key_value_config_sets_expected_values() {
        let config = config();
        let kv_config = key_value_config(&config);

        assert_eq!(kv_config.bucket, "bucket");
        assert_eq!(kv_config.history, 1);
        assert_eq!(kv_config.max_age, Duration::ZERO);
        assert_eq!(kv_config.limit_markers, Some(Duration::from_secs(10)));
    }

    #[tokio::test]
    async fn provision_store_uses_create_on_success() {
        let config = config();
        let store = MockJetStreamKvStore::new();
        apply_settings(&store, strict_settings(config.ttl()));

        let client = MockJetStreamKvClient::new();
        client.set_create_result(store);

        let result = provision_store(&client, &config).await;
        assert!(result.is_ok());
        assert!(client.requested_buckets().is_empty());
        assert_eq!(client.create_configs()[0].bucket, "bucket");
    }

    #[tokio::test]
    async fn provision_store_falls_back_to_get_on_already_exists() {
        let config = config();
        let store = MockJetStreamKvStore::new();
        apply_settings(&store, strict_settings(config.ttl()));

        let client = MockJetStreamKvClient::new();
        client.fail_create_already_exists();
        client.set_get_result(store);

        let result = provision_store(&client, &config).await;
        assert!(result.is_ok());
        assert_eq!(client.requested_buckets(), vec!["bucket".to_string()]);
    }

    #[tokio::test]
    async fn provision_store_surfaces_get_errors_after_already_exists() {
        let config = config();
        let client = MockJetStreamKvClient::new();
        client.fail_create_already_exists();
        client.fail_get(async_nats::jetstream::context::KeyValueErrorKind::GetBucket);

        let error = provision_store::<_, MockJetStreamKvStore>(&client, &config)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to open existing lease bucket")
        );
    }

    #[tokio::test]
    async fn provision_store_surfaces_create_errors() {
        let config = config();
        let client = MockJetStreamKvClient::new();
        client.fail_create(CreateKeyValueErrorKind::TimedOut);

        let error = provision_store::<_, MockJetStreamKvStore>(&client, &config)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("failed to create lease bucket"));
    }

    #[tokio::test]
    async fn provision_store_surfaces_status_errors() {
        let config = config();
        let store = MockJetStreamKvStore::new();
        store.fail_status(kv::StatusErrorKind::TimedOut);

        let client = MockJetStreamKvClient::new();
        client.set_create_result(store);

        let error = provision_store::<_, MockJetStreamKvStore>(&client, &config)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to inspect lease bucket configuration")
        );
    }

    #[tokio::test]
    async fn provision_store_rejects_incompatible_bucket_settings() {
        let config = config();
        let store = MockJetStreamKvStore::new();
        apply_settings(
            &store,
            KeyValueSettings {
                history: 2,
                ..strict_settings(config.ttl())
            },
        );

        let client = MockJetStreamKvClient::new();
        client.set_create_result(store);

        let error = provision_store::<_, MockJetStreamKvStore>(&client, &config)
            .await
            .unwrap_err();
        assert!(matches!(error, LeaseError::IncompatibleBucketConfig { .. }));
    }
}
