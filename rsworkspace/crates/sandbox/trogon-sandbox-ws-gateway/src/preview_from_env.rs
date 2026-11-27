use std::sync::Arc;
use std::time::Duration;

use trogon_std::env::ReadEnv;

use crate::constants::{PREVIEW_ACTIVE_KEY_ID_ENV, PREVIEW_SIGNING_KEYS_ENV};
use crate::{PreviewConfigError, PreviewKeys, PreviewState};

/// Preview URLs are enabled only when `SANDBOX_PREVIEW_SIGNING_KEYS` (`kid=hexkey[,kid=hexkey...]`) is set.
/// `SANDBOX_PREVIEW_ACTIVE_KEY_ID` picks the signing key; the others stay valid for verification. The key material
/// is read here and never logged.
pub fn preview_from_env<E: ReadEnv>(
    env: &E,
    public_url: Option<&str>,
    ttl: Duration,
    max_ttl: Duration,
) -> Result<Option<Arc<PreviewState>>, PreviewConfigError> {
    let Ok(spec) = env.var(PREVIEW_SIGNING_KEYS_ENV) else {
        tracing::info!(env = PREVIEW_SIGNING_KEYS_ENV, "preview access disabled");
        return Ok(None);
    };
    let active = env.var(PREVIEW_ACTIVE_KEY_ID_ENV).ok();
    let keys = PreviewKeys::parse(&spec, active.as_deref())?;
    let public_url = public_url
        .filter(|url| !url.is_empty())
        .ok_or(PreviewConfigError::MissingPublicUrl)?;
    tracing::info!("preview access enabled");
    Ok(Some(Arc::new(PreviewState::new(keys, public_url, ttl, max_ttl)?)))
}
