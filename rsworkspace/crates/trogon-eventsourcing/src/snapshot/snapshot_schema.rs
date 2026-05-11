use super::SnapshotStoreConfig;

pub trait SnapshotSchema {
    const SNAPSHOT_STREAM_PREFIX: &'static str;
    const CHECKPOINT_NAME: Option<&'static str> = None;

    fn snapshot_store_config() -> SnapshotStoreConfig
    where
        Self: Sized,
    {
        SnapshotStoreConfig::new(Self::SNAPSHOT_STREAM_PREFIX, Self::CHECKPOINT_NAME)
    }
}
