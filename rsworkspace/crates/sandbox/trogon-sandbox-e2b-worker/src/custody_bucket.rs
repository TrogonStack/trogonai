use async_nats::jetstream::{self, kv, stream};

use crate::custody_bucket_error::CustodyBucketError;

/// Opens the bucket `config` names, creating it when absent, and refuses one that could expire or evict records.
pub(crate) async fn open_custody_bucket(
    context: &jetstream::Context,
    config: kv::Config,
) -> Result<kv::Store, CustodyBucketError> {
    let bucket = config.bucket.clone();
    let store = match context.get_key_value(&bucket).await {
        Ok(store) => store,
        Err(_) => match context.create_key_value(config).await {
            Ok(store) => store,
            Err(create) => {
                return context
                    .get_key_value(&bucket)
                    .await
                    .map_err(|load| CustodyBucketError::Unavailable { create, load });
            }
        },
    };
    ensure_durable(&store).await?;
    Ok(store)
}

/// Refuses a bucket whose stream could expire or evict records, so a stream changed after it was opened is caught.
pub(crate) async fn ensure_durable(store: &kv::Store) -> Result<(), CustodyBucketError> {
    let mut stream = store.stream.clone();
    let info = stream.info().await?;
    if !matches!(
        &info.config,
        stream::Config {
            max_age,
            discard: stream::DiscardPolicy::New,
            retention: stream::RetentionPolicy::Limits,
            storage: stream::StorageType::File,
            allow_message_ttl: false,
            mirror: None,
            sources,
            ..
        } if max_age.is_zero() && sources.as_ref().is_none_or(Vec::is_empty)
    ) {
        return Err(CustodyBucketError::NotDurable);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
