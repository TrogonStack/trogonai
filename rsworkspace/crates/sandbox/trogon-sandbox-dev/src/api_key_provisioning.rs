use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::SystemTime;

use bytes::Bytes;
use rand_core::{OsRng, RngCore};
use trogon_sandbox_auth::{ApiKeyRecord, BearerToken, NatsKvApiKeyStore};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use crate::DevError;

/// A random `sbx-dev-` key for a throwaway local stack.
pub fn generate_api_key() -> Result<BearerToken, DevError> {
    let mut raw = [0u8; 16];
    OsRng.try_fill_bytes(&mut raw).map_err(DevError::GenerateApiKey)?;
    let hex: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(BearerToken::new(format!("sbx-dev-{hex}"))?)
}

/// Writes `token` to a new owner-only file. An existing file or symlink at `path` is refused, never overwritten.
pub fn write_api_key_file(path: &Path, token: &BearerToken) -> Result<(), DevError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    #[cfg(unix)]
    options.mode(0o600);
    let file_error = |source: std::io::Error| DevError::ApiKeyFile {
        path: path.to_path_buf(),
        source,
    };
    let mut file = options.open(path).map_err(file_error)?;
    file.write_all(token.expose_secret().as_bytes()).map_err(file_error)?;
    file.sync_all().map_err(file_error)
}

/// Registers `token` as an API key for `tenant` in the bucket the gateway reads, so a session request that presents it
/// is accepted.
pub async fn register_api_key(nats_url: &str, tenant: &TenantId, token: &BearerToken) -> Result<(), DevError> {
    let client = async_nats::connect(nats_url).await.map_err(DevError::NatsConnect)?;
    let store = NatsKvApiKeyStore::new(async_nats::jetstream::new(client))
        .await
        .map_err(DevError::RegisterApiKey)?;
    let record = ApiKeyRecord::new(
        tenant.clone(),
        ServiceAccountId::new("dev"),
        "trogon-sandbox-dev",
        SystemTime::now(),
    );
    let bytes = serde_json::to_vec(&record).map_err(DevError::EncodeApiKey)?;
    store
        .kv()
        .put(token.hash().as_str(), Bytes::from(bytes))
        .await
        .map_err(DevError::StoreApiKey)?;
    Ok(())
}
