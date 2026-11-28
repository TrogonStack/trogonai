use std::env::VarError;
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
    let Some(spec) = optional_var(env, PREVIEW_SIGNING_KEYS_ENV)? else {
        tracing::info!(env = PREVIEW_SIGNING_KEYS_ENV, "preview access disabled");
        return Ok(None);
    };
    let active = optional_var(env, PREVIEW_ACTIVE_KEY_ID_ENV)?;
    let keys = PreviewKeys::parse(&spec, active.as_deref())?;
    let public_url = public_url
        .filter(|url| !url.is_empty())
        .ok_or(PreviewConfigError::MissingPublicUrl)?;
    tracing::info!("preview access enabled");
    Ok(Some(Arc::new(PreviewState::new(keys, public_url, ttl, max_ttl)?)))
}

/// The value of `name`, `None` when it is unset. A value that is not valid Unicode is refused instead of being read as
/// unset, which would turn previews off or pick another signing key.
fn optional_var<E: ReadEnv>(env: &E, name: &'static str) -> Result<Option<String>, PreviewConfigError> {
    match env.var(name) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(PreviewConfigError::NotUnicode { name }),
    }
}
