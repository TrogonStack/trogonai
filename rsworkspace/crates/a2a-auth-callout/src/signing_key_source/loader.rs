use std::sync::Arc;

use crate::error::AuthCalloutError;

use super::{EnvSigningKeySource, FileSigningKeySource, SigningKeySource};

pub fn signing_key_source_from_process_env() -> Result<Arc<dyn SigningKeySource>, AuthCalloutError> {
    let kind = std::env::var("AUTH_CALLOUT_SIGNING_KEY_SOURCE")
        .unwrap_or_else(|_| "env".into());
    match kind.as_str() {
        "env" => {
            if std::env::var("AUTH_CALLOUT_SIGNING_SECRET").is_err() {
                let fallback = std::env::var("AUTH_CALLOUT_ISSUER_NKEY_SEED").map_err(|_| {
                    AuthCalloutError::Internal(
                        "AUTH_CALLOUT_SIGNING_SECRET or AUTH_CALLOUT_ISSUER_NKEY_SEED is required for env custody"
                            .into(),
                    )
                })?;
                unsafe {
                    std::env::set_var("AUTH_CALLOUT_SIGNING_SECRET", fallback);
                }
            }
            Ok(Arc::new(EnvSigningKeySource::from_env()?))
        }
        "file" => {
            let current = std::env::var("AUTH_CALLOUT_SIGNING_KEY_PATH").map_err(|_| {
                AuthCalloutError::Internal(
                    "AUTH_CALLOUT_SIGNING_KEY_PATH is required when AUTH_CALLOUT_SIGNING_KEY_SOURCE=file"
                        .into(),
                )
            })?;
            let previous = std::env::var("AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH").ok();
            Ok(Arc::new(FileSigningKeySource::new(
                current,
                previous.as_deref(),
            )?))
        }
        "vault" => Err(AuthCalloutError::Internal(
            "AUTH_CALLOUT_SIGNING_KEY_SOURCE=vault is not wired yet; use file or env".into(),
        )),
        other => Err(AuthCalloutError::Internal(format!(
            "unknown AUTH_CALLOUT_SIGNING_KEY_SOURCE: {other} (expected env, file, or vault)"
        ))),
    }
}
