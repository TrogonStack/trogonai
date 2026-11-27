//! Builds the authentication context from the command line.

mod auth_setup_error;

use std::sync::Arc;

use tracing::info;
use trogon_sandbox_auth::{
    InMemoryApiKeyStore, NatsKvApiKeyStore, NoopAuthorizer, SpiceDbAuthorizer, SpiceDbEndpoint, SpiceDbToken,
};

use crate::auth::AuthContext;
use crate::cli::Cli;

pub use auth_setup_error::AuthSetupError;

/// Connects the permission service and API key store when authz is enforced, and falls back to
/// the always-allow context otherwise.
pub async fn connect(cli: &Cli, nats: async_nats::Client) -> Result<AuthContext, AuthSetupError> {
    if !cli.authz_enforce {
        return Ok(AuthContext {
            api_keys: Arc::new(InMemoryApiKeyStore::new()),
            authorizer: Arc::new(NoopAuthorizer),
            enforce: false,
        });
    }
    let endpoint = SpiceDbEndpoint::new(cli.spicedb_endpoint.as_deref().ok_or(AuthSetupError::MissingEndpoint)?)?;
    let token = SpiceDbToken::new(cli.spicedb_token.as_deref().ok_or(AuthSetupError::MissingToken)?)?;
    let authz =
        SpiceDbAuthorizer::connect_with_resource_type(&endpoint, &token, cli.spicedb_tenant_resource_type.clone())
            .await?;
    info!(endpoint = %endpoint, "SpiceDB authorizer connected");

    let jetstream = async_nats::jetstream::new(nats);
    let api_keys = NatsKvApiKeyStore::new_with_bucket(jetstream, &cli.api_key_bucket).await?;
    info!(bucket = %cli.api_key_bucket, "NATS KV API key store ready");

    Ok(AuthContext {
        api_keys: Arc::new(api_keys),
        authorizer: Arc::new(authz),
        enforce: true,
    })
}

#[cfg(test)]
mod tests;
