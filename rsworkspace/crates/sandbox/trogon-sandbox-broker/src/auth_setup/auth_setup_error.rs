use trogon_sandbox_auth::{ApiKeyStoreError, SpiceDbConnectError, SpiceDbEndpointError, SpiceDbTokenError};

/// Building the authentication context failed.
#[derive(Debug, thiserror::Error)]
pub enum AuthSetupError {
    #[error(
        "--spicedb-endpoint / SPICEDB_ENDPOINT is required when authz is enforced (the default); pass --insecure-dev for a local stack"
    )]
    MissingEndpoint,
    #[error(
        "--spicedb-token / SPICEDB_TOKEN is required when authz is enforced (the default); pass --insecure-dev for a local stack"
    )]
    MissingToken,
    #[error(transparent)]
    Endpoint(#[from] SpiceDbEndpointError),
    #[error(transparent)]
    Token(#[from] SpiceDbTokenError),
    #[error(transparent)]
    Connect(#[from] SpiceDbConnectError),
    #[error(transparent)]
    ApiKeys(#[from] ApiKeyStoreError),
}
