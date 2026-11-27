use std::sync::Arc;

use trogon_sandbox_protocol::DispatchSigner;
use trogon_std::env::ReadEnv;

use crate::CheckFailureError;

/// Reads the signer from `env` using the broker's key variables: `SANDBOX_DISPATCH_SIGNING_KEY_FILE` (a file holding the base64
/// seed) or `SANDBOX_DISPATCH_SIGNING_KEY`, plus the optional `SANDBOX_DISPATCH_SIGNING_KEY_ID` and
/// `SANDBOX_DISPATCH_TTL_SECS`. `Ok(None)` when no key is set, so the suite runs unsigned.
pub fn signer_from_env(env: &impl ReadEnv) -> Result<Option<Arc<DispatchSigner>>, CheckFailureError> {
    Ok(DispatchSigner::from_env(env)?.map(Arc::new))
}

#[cfg(test)]
mod tests;
