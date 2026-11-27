use std::sync::{Arc, Mutex, PoisonError};

use reqwest::header::HeaderValue;
use url::Url;

use super::EnvdTargetError;
use super::constants::ENVD_PORT;
use crate::api::E2bConfig;

/// How to reach one sandbox's envd. The token is not part of `Debug`.
#[derive(Clone)]
pub struct EnvdTarget {
    pub base: Url,
    token: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for EnvdTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvdTarget")
            .field("base", &self.base)
            .field(
                "token",
                &self
                    .token
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl EnvdTarget {
    /// The envd address of `sandbox_id`: `https://49983-<id>.<domain>`, unless the
    /// configuration pins one origin for every sandbox.
    pub fn for_sandbox(
        config: &E2bConfig,
        sandbox_id: &str,
        domain: Option<&str>,
        token: Option<String>,
    ) -> Result<Self, EnvdTargetError> {
        let base = match &config.envd_url {
            Some(url) => url.clone(),
            None => {
                let domain = domain.unwrap_or(&config.domain);
                Url::parse(&format!("https://{ENVD_PORT}-{sandbox_id}.{domain}"))?
            }
        };
        Ok(Self {
            base,
            token: Arc::new(Mutex::new(token)),
        })
    }

    pub fn replace_token(&mut self, token: Option<String>) {
        *self.token.lock().unwrap_or_else(PoisonError::into_inner) = token;
    }

    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        self.base == other.base && Arc::ptr_eq(&self.token, &other.token)
    }

    /// The access token as a sensitive header value, when there is one that fits a header.
    pub(super) fn access_header(&self) -> Option<HeaderValue> {
        let token = self.token.lock().unwrap_or_else(PoisonError::into_inner);
        let mut value = HeaderValue::from_str(token.as_deref()?).ok()?;
        value.set_sensitive(true);
        Some(value)
    }

    /// The URL of `path` on this envd.
    pub(super) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base.as_str().trim_end_matches('/'))
    }
}
