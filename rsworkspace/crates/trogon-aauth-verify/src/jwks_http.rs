//! HTTP well-known JWKS resolver.
//!
//! Fetches `{iss}/.well-known/{dwk}` per the AAuth draft's discovery
//! mechanism. `iss` is the only input the `JwksResolver` trait hands us, so
//! this resolver is configured with an ordered list of well-known filenames
//! to try and returns the first one that parses. `iss` is attacker-influenced
//! (it comes from an unverified JWT claim before signature checking), so the
//! resolver never trusts it beyond building a URL: it rejects non-https
//! issuers outright and bounds both the read timeout and response body size
//! so a hostile or slow endpoint cannot stall or exhaust verification.

use std::time::Duration;

use trogon_identity_types::aauth::{DWK_AGENT, DWK_PERSON, DWK_RESOURCE};

use crate::jwks::{JwksError, JwksResolver};

/// Default cap on a single well-known JWKS response body. JWKS documents are
/// small (a handful of public keys); this is generous headroom while still
/// refusing an issuer that tries to stream gigabytes at the verifier.
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 256 * 1024;
/// Default request timeout for a single well-known fetch attempt.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Non-zero response-size cap for a single well-known JWKS fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxResponseBytes(u64);

impl MaxResponseBytes {
    pub fn new(bytes: u64) -> Result<Self, MaxResponseBytesError> {
        if bytes == 0 {
            return Err(MaxResponseBytesError::Zero);
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl Default for MaxResponseBytes {
    fn default() -> Self {
        Self(DEFAULT_MAX_RESPONSE_BYTES)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MaxResponseBytesError {
    #[error("max response bytes must be non-zero")]
    Zero,
}

/// Non-zero request timeout for a single well-known JWKS fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestTimeout(Duration);

impl RequestTimeout {
    pub fn new(duration: Duration) -> Result<Self, RequestTimeoutError> {
        if duration.is_zero() {
            return Err(RequestTimeoutError::Zero);
        }
        Ok(Self(duration))
    }

    #[must_use]
    pub fn get(self) -> Duration {
        self.0
    }
}

impl Default for RequestTimeout {
    fn default() -> Self {
        Self(DEFAULT_REQUEST_TIMEOUT)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RequestTimeoutError {
    #[error("request timeout must be non-zero")]
    Zero,
}

/// A single `dwk` (discoverable-well-known) filename tried against
/// `{iss}/.well-known/{dwk}`. Wrapping the primitive stops a caller from
/// passing an already-joined path or a leading slash by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WellKnownDwk(String);

impl WellKnownDwk {
    pub fn new(raw: impl Into<String>) -> Result<Self, WellKnownDwkError> {
        let value = raw.into();
        if value.trim().is_empty() {
            return Err(WellKnownDwkError::Empty);
        }
        if value.contains('/') {
            return Err(WellKnownDwkError::ContainsSlash(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WellKnownDwkError {
    #[error("well-known dwk filename must not be empty")]
    Empty,
    #[error("well-known dwk filename must not contain '/': {0:?}")]
    ContainsSlash(String),
}

/// Builds the default `dwk` fallback order from the built-in AAuth constants.
#[allow(clippy::expect_used)]
fn default_dwk_order() -> Vec<WellKnownDwk> {
    [DWK_AGENT, DWK_RESOURCE, DWK_PERSON]
        .into_iter()
        // DWK_AGENT/DWK_RESOURCE/DWK_PERSON are non-empty, slash-free filename
        // constants -- always valid WellKnownDwk values.
        .map(|dwk| WellKnownDwk::new(dwk).expect("built-in DWK constant is a valid filename"))
        .collect()
}

/// Resolves `iss -> JwkSet` by fetching `{iss}/.well-known/{dwk}` for each
/// configured `dwk` filename, in order, returning the first one that parses.
///
/// HTTPS-only: the AAuth draft requires HTTPS issuers, and `iss` is
/// attacker-influenced (read from an unverified JWT claim before signature
/// checking), so plaintext HTTP is rejected before any network call is made.
pub struct HttpJwksResolver {
    client: reqwest::Client,
    dwk_order: Vec<WellKnownDwk>,
    max_response_bytes: MaxResponseBytes,
}

impl HttpJwksResolver {
    /// Builds a resolver trying [`DWK_AGENT`], [`DWK_RESOURCE`], then
    /// [`DWK_PERSON`] in order, with the default timeout and response cap.
    ///
    /// # Panics
    ///
    /// Panics if the underlying `reqwest::Client` fails to build (e.g. TLS
    /// backend initialization failure) -- that indicates a broken runtime
    /// environment, not a recoverable configuration error.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(
            default_dwk_order(),
            RequestTimeout::default(),
            MaxResponseBytes::default(),
        )
    }

    /// Builds a resolver with an explicit `dwk` fallback order, request
    /// timeout, and response size cap.
    ///
    /// # Panics
    ///
    /// Panics if the underlying `reqwest::Client` fails to build.
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn with_config(
        dwk_order: Vec<WellKnownDwk>,
        timeout: RequestTimeout,
        max_response_bytes: MaxResponseBytes,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout.get())
            .build()
            .expect("reqwest client with rustls-tls backend must build");
        Self {
            client,
            dwk_order,
            max_response_bytes,
        }
    }
}

impl Default for HttpJwksResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl JwksResolver for HttpJwksResolver {
    async fn resolve(&self, iss: &str) -> Result<jsonwebtoken::jwk::JwkSet, JwksError> {
        let base = require_https_issuer(iss)?;
        self.resolve_from_base(iss, base).await
    }
}

impl HttpJwksResolver {
    /// Shared by `resolve` (which gates `base` behind [`require_https_issuer`])
    /// and tests (which exercise the fetch/ordering logic against a
    /// plain-HTTP `wiremock` server, where enforcing HTTPS would be
    /// untestable without standing up TLS in the test harness).
    async fn resolve_from_base(&self, iss: &str, base: &str) -> Result<jsonwebtoken::jwk::JwkSet, JwksError> {
        let mut attempts = Vec::with_capacity(self.dwk_order.len());
        for dwk in &self.dwk_order {
            let url = well_known_url(base, dwk);
            match fetch_jwks(&self.client, &url, self.max_response_bytes).await {
                Ok(set) => return Ok(set),
                Err(reason) => attempts.push(format!("{url}: {reason}")),
            }
        }
        Err(JwksError::Transport(format!(
            "no well-known JWKS document resolved for issuer {iss:?}; tried [{}]",
            attempts.join(", ")
        )))
    }
}

/// Validates `iss` is an `https://` URL and returns it with any trailing
/// slash stripped so `well_known_url` can join a single `/` deterministically.
fn require_https_issuer(iss: &str) -> Result<&str, JwksError> {
    if !iss.starts_with("https://") {
        return Err(JwksError::Transport(format!(
            "issuer must be an https:// URL, got {iss:?}"
        )));
    }
    Ok(iss.trim_end_matches('/'))
}

fn well_known_url(base: &str, dwk: &WellKnownDwk) -> String {
    format!("{base}/.well-known/{}", dwk.as_str())
}

async fn fetch_jwks(
    client: &reqwest::Client,
    url: &str,
    max_response_bytes: MaxResponseBytes,
) -> Result<jsonwebtoken::jwk::JwkSet, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("status {status}"));
    }
    if let Some(len) = response.content_length()
        && len > max_response_bytes.get()
    {
        return Err(format!(
            "response Content-Length {len} exceeds cap {}",
            max_response_bytes.get()
        ));
    }
    let bytes = response.bytes().await.map_err(|e| format!("body read failed: {e}"))?;
    if bytes.len() as u64 > max_response_bytes.get() {
        return Err(format!(
            "response body {} bytes exceeds cap {}",
            bytes.len(),
            max_response_bytes.get()
        ));
    }
    serde_json::from_slice::<jsonwebtoken::jwk::JwkSet>(&bytes).map_err(|e| format!("malformed JWKS JSON: {e}"))
}

#[cfg(test)]
mod tests;
