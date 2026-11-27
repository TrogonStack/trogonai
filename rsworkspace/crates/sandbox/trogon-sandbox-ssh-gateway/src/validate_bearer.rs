use trogon_sandbox_auth::{ApiKeyStore, ApiKeyStoreError, BearerToken};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

/// The tenant and service account `bearer` authenticates as, or `None` when it is not a known API key.
pub async fn validate_bearer(
    api_keys: &dyn ApiKeyStore,
    bearer: &str,
) -> Result<Option<(TenantId, ServiceAccountId)>, ApiKeyStoreError> {
    let Ok(token) = BearerToken::new(bearer) else {
        return Ok(None);
    };
    let record = api_keys.lookup(&token.hash()).await?;
    Ok(record.map(|record| (record.tenant().clone(), record.service_account().clone())))
}
