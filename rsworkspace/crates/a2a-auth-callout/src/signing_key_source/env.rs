use std::sync::Once;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use tracing::warn;

#[cfg(test)]
static ENV_DEV_WARN_COUNT: AtomicUsize = AtomicUsize::new(0);

use crate::error::AuthCalloutError;
use crate::jwt::SigningKey;

use super::key_version::KeyVersion;
use super::signing_key_handle::SigningKeyHandle;
use super::SigningKeySource;

static ENV_DEV_WARN_ONCE: Once = Once::new();

const VERSION_CURRENT: &str = "current";
const VERSION_PREVIOUS: &str = "previous";

#[derive(Debug)]
pub struct EnvSigningKeySource {
    current: SigningKeyHandle,
    previous: Option<SigningKeyHandle>,
}

impl EnvSigningKeySource {
    pub fn from_env() -> Result<Self, AuthCalloutError> {
        ENV_DEV_WARN_ONCE.call_once(|| {
            #[cfg(test)]
            ENV_DEV_WARN_COUNT.fetch_add(1, Ordering::SeqCst);
            warn!(
                "AUTH_CALLOUT_SIGNING_SECRET env custody is dev-only; use file or vault in production"
            );
        });

        let current_secret = std::env::var("AUTH_CALLOUT_SIGNING_SECRET").map_err(|_| {
            AuthCalloutError::Internal("AUTH_CALLOUT_SIGNING_SECRET is required for env custody".into())
        })?;
        let current = SigningKeyHandle::new(
            KeyVersion::new(VERSION_CURRENT).expect("static version"),
            SigningKey::from_secret(current_secret.as_bytes()),
        );

        let previous = std::env::var("AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|secret| {
                SigningKeyHandle::new(
                    KeyVersion::new(VERSION_PREVIOUS).expect("static version"),
                    SigningKey::from_secret(secret.as_bytes()),
                )
            });

        Ok(Self { current, previous })
    }
}

impl SigningKeySource for EnvSigningKeySource {
    fn current(&self) -> SigningKeyHandle {
        self.current.clone()
    }

    fn accepted(&self) -> Vec<SigningKeyHandle> {
        let mut keys = vec![self.current.clone()];
        if let Some(prev) = &self.previous {
            keys.push(prev.clone());
        }
        keys
    }
}

#[cfg(test)]
pub(crate) fn test_env_dev_warn_count() -> usize {
    ENV_DEV_WARN_COUNT.load(Ordering::SeqCst)
}
