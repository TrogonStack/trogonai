use reqwest::header::HeaderValue;
use url::Url;

use super::E2bConfigError;

/// Where the worker reaches E2B. The API key is not part of `Debug`.
#[derive(Clone)]
pub struct E2bConfig {
    pub api_url: Url,
    /// Domain sandboxes are served under when E2B does not report one.
    pub domain: String,
    api_key: HeaderValue,
    /// Replaces the `https://49983-<id>.<domain>` envd address. For self-hosted or
    /// test deployments that serve every sandbox from one origin.
    pub envd_url: Option<Url>,
}

impl std::fmt::Debug for E2bConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2bConfig")
            .field("api_url", &self.api_url)
            .field("domain", &self.domain)
            .field("api_key", &"<redacted>")
            .field("envd_url", &self.envd_url)
            .finish()
    }
}

fn is_loopback(url: &Url) -> bool {
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1" | "[::1]"))
}

fn validate_endpoint(url: &Url) -> Result<(), E2bConfigError> {
    let secure = url.scheme() == "https" || (url.scheme() == "http" && is_loopback(url));
    if !secure {
        return Err(E2bConfigError::InsecureEndpoint);
    }
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(E2bConfigError::DecoratedEndpoint);
    }
    Ok(())
}

/// Accepts a non-empty run of ASCII letters, digits, `-` and `.`.
pub(crate) fn validate_domain(domain: &str) -> Result<(), E2bConfigError> {
    let valid = !domain.is_empty()
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(E2bConfigError::InvalidDomain)
    }
}

impl E2bConfig {
    pub fn new(api_url: Url, domain: impl Into<String>, api_key: impl AsRef<str>) -> Result<Self, E2bConfigError> {
        validate_endpoint(&api_url)?;
        let domain = domain.into();
        validate_domain(&domain)?;
        let api_key = api_key.as_ref();
        if api_key.is_empty() {
            return Err(E2bConfigError::EmptyApiKey);
        }
        let mut api_key = HeaderValue::from_str(api_key).map_err(|_| E2bConfigError::InvalidApiKey)?;
        api_key.set_sensitive(true);
        Ok(Self {
            api_url,
            domain,
            api_key,
            envd_url: None,
        })
    }

    pub fn with_envd_url(mut self, url: Url) -> Result<Self, E2bConfigError> {
        validate_endpoint(&url)?;
        self.envd_url = Some(url);
        Ok(self)
    }

    pub(crate) fn api_key_header(&self) -> HeaderValue {
        self.api_key.clone()
    }
}
