use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjectorError {
    #[error("decode envelope: {0}")]
    Decode(String),
    #[error("normalize envelope: {0}")]
    Normalize(String),
    #[error("index event: {0}")]
    Index(#[from] IndexerError),
    #[error("jetstream consumer: {0}")]
    Consumer(String),
}

#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migrate: {0}")]
    Migrate(String),
    #[error("query: {0}")]
    Query(String),
}

#[derive(Debug, Error)]
pub enum TrafficViewError {
    #[error("index error: {0}")]
    Index(#[from] IndexerError),
    #[error("projector error: {0}")]
    Projector(#[from] ProjectorError),
    #[error("siem export error: {0}")]
    Siem(String),
}
