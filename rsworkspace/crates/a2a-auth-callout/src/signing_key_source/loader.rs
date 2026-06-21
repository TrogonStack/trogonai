use std::sync::Arc;

use crate::error::AuthCalloutError;

use super::{EnvSigningKeySource, FileSigningKeySource, SigningKeySource};

pub fn signing_key_source_from_process_env() -> Result<Arc<dyn SigningKeySource>, AuthCalloutError> {
    let kind = std::env::var("AUTH_CALLOUT_SIGNING_KEY_SOURCE").unwrap_or_else(|_| "env".into());
    match kind.as_str() {
        "env" => {
            // Resolve the secret in the loader (with the AUTH_CALLOUT_ISSUER_NKEY_SEED
            // legacy fallback) instead of mutating the process environment from a
            // config loader — `std::env::set_var` is unsafe and races with any
            // other thread reading env vars in this process.
            let secret = std::env::var("AUTH_CALLOUT_SIGNING_SECRET")
                .or_else(|_| std::env::var("AUTH_CALLOUT_ISSUER_NKEY_SEED"))
                .map_err(|_| AuthCalloutError::MissingEnvVar("AUTH_CALLOUT_SIGNING_SECRET"))?;
            Ok(Arc::new(EnvSigningKeySource::from_secrets(secret)?))
        }
        "file" => {
            let current = std::env::var("AUTH_CALLOUT_SIGNING_KEY_PATH")
                .map_err(|_| AuthCalloutError::MissingEnvVar("AUTH_CALLOUT_SIGNING_KEY_PATH"))?;
            // Treat an unset *or* empty `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH`
            // as "no previous key" — same convention the env source uses for
            // `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS`, so operators don't have
            // to delete the var to disable the previous slot.
            let previous = std::env::var("AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH")
                .ok()
                .filter(|s| !s.is_empty());
            Ok(Arc::new(FileSigningKeySource::new(current, previous.as_deref())?))
        }
        "vault" => Err(AuthCalloutError::VaultNotConfigured),
        other => Err(AuthCalloutError::UnknownSigningKeySource(other.to_string())),
    }
}
