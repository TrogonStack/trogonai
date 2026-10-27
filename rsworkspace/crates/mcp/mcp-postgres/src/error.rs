#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration")]
    Configuration,
    #[error("unsupported or invalid SQL")]
    UnsupportedSql,
    #[error("SQL parsing failed")]
    Parse(#[from] pg_query::Error),
    #[error("unknown connection")]
    UnknownConnection,
    #[error("access denied")]
    Denied,
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("authorization service failed")]
    Authorization(#[from] tonic::Status),
    #[error("authorization connection failed")]
    AuthorizationConnection(#[from] tonic::transport::Error),
    #[error("resource limit exceeded")]
    Limit,
    #[error("request timed out")]
    Timeout,
    #[error("unsupported database object")]
    UnsupportedObject,
    #[error("invalid identifier")]
    Identifier,
    #[error("invalid object ID")]
    ObjectId,
}
