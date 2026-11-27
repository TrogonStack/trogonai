/// Why a drive's stored data could not be purged.
#[derive(Debug, thiserror::Error)]
pub enum DrivePurgeError {
    #[error("provider-owned volumes must be deleted through their provider; the drive registration is retained")]
    ProviderOwned,
    #[error("build s3 client for bucket '{bucket}': {source}")]
    Client {
        bucket: String,
        source: object_store::Error,
    },
    #[error("list objects under '{prefix}': {source}")]
    List {
        prefix: String,
        source: object_store::Error,
    },
    #[error("delete object '{location}': {source}")]
    Delete {
        location: String,
        source: object_store::Error,
    },
}
