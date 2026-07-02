//! CVE feed gate for registered MCP server packages (WI-05).
//!
//! Performs OSV.dev lookups per registered package/version, with a
//! 1-hour result cache keyed on an injectable clock, and fail-closed
//! semantics when the feed is unreachable or returns a response that
//! cannot be parsed.
//!
//! The service layer (WI-01) calls [`CveFeedGate::check`] before
//! forwarding a `tools/call` to a registered MCP server; anything other
//! than [`FeedVerdict::Allow`] must block dispatch.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_std::time::{GetElapsed, GetNow};

use crate::{
    CveFeedCacheEntry, CveFeedUnreachablePolicy, DenyUnknownReason, FeedVerdict, OsvQueryWire, OsvResponseWire,
    PackageCoordinate,
};

/// Default OSV.dev query endpoint.
pub const OSV_API_URL: &str = "https://api.osv.dev/v1/query";

/// Default cache TTL: 1 hour, matching AGT's `cache_ttl_seconds` default.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Pre-dispatch CVE gate check for a single registered MCP server package.
/// One operation per trait, so the service layer can depend on this
/// without pulling in registration/tracking concerns.
#[async_trait]
pub trait CveFeedGate: Send + Sync {
    async fn check(&self, package: &PackageCoordinate) -> FeedVerdict;
}

/// OSV.dev-backed implementation of [`CveFeedGate`].
///
/// Generic over any clock implementing `trogon_std::time::{GetNow,
/// GetElapsed}` so production wires `SystemClock` and tests wire
/// `MockClock` to control cache expiry deterministically.
pub struct OsvCveFeedGate<C: GetNow> {
    client: reqwest::Client,
    api_url: String,
    clock: C,
    cache_ttl: Duration,
    unreachable_policy: CveFeedUnreachablePolicy,
    cache: Mutex<HashMap<String, CveFeedCacheEntry<C::Instant>>>,
}

impl<C: GetNow + GetElapsed> OsvCveFeedGate<C> {
    pub fn new(client: reqwest::Client, clock: C) -> Self {
        Self {
            client,
            api_url: OSV_API_URL.to_string(),
            clock,
            cache_ttl: DEFAULT_CACHE_TTL,
            unreachable_policy: CveFeedUnreachablePolicy::Deny,
            cache: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn with_api_url(mut self, api_url: impl Into<String>) -> Self {
        self.api_url = api_url.into();
        self
    }

    #[must_use]
    pub fn with_cache_ttl(mut self, cache_ttl: Duration) -> Self {
        self.cache_ttl = cache_ttl;
        self
    }

    /// Fail-open is an explicit, deliberate opt-in; the gate defaults to
    /// fail-closed (`CveFeedUnreachablePolicy::Deny`) and stays there
    /// unless a caller calls this on purpose.
    #[must_use]
    pub fn with_unreachable_policy(mut self, policy: CveFeedUnreachablePolicy) -> Self {
        self.unreachable_policy = policy;
        self
    }

    async fn query_osv(&self, package: &PackageCoordinate) -> FeedVerdict {
        let body = OsvQueryWire::from_coordinate(package);
        let response = match self.client.post(&self.api_url).json(&body).send().await {
            Ok(response) => response,
            Err(err) => return self.unreachable_verdict(DenyUnknownReason::FeedUnreachable(err.to_string())),
        };

        let bytes = match response.error_for_status() {
            Ok(response) => match response.bytes().await {
                Ok(bytes) => bytes,
                Err(err) => return self.unreachable_verdict(DenyUnknownReason::FeedUnreachable(err.to_string())),
            },
            Err(err) => return self.unreachable_verdict(DenyUnknownReason::FeedUnreachable(err.to_string())),
        };

        match OsvResponseWire::parse(&bytes) {
            Ok(parsed) => {
                let records = parsed.into_vulnerability_records();
                if records.is_empty() {
                    FeedVerdict::Allow
                } else {
                    FeedVerdict::DenyVulnerable(records)
                }
            }
            Err(err) => self.unreachable_verdict(DenyUnknownReason::MalformedResponse(err.to_string())),
        }
    }

    /// Collapse a feed failure into a verdict per the configured policy.
    /// `Deny` (the default) is the only fail-closed choice; `Allow` is
    /// available only because a caller explicitly asked for fail-open via
    /// [`Self::with_unreachable_policy`].
    fn unreachable_verdict(&self, reason: DenyUnknownReason) -> FeedVerdict {
        if self.unreachable_policy.is_fail_closed() {
            FeedVerdict::DenyUnknown(reason)
        } else {
            FeedVerdict::Allow
        }
    }
}

#[async_trait]
impl<C> CveFeedGate for OsvCveFeedGate<C>
where
    C: GetNow + GetElapsed + Send + Sync,
    C::Instant: Send,
{
    async fn check(&self, package: &PackageCoordinate) -> FeedVerdict {
        let cache_key = package.cache_key();

        {
            let cache = self.cache.lock().await;
            if let Some(entry) = cache.get(&cache_key)
                && entry.is_fresh(&self.clock, self.cache_ttl)
            {
                return entry.verdict().clone();
            }
        }

        let verdict = self.query_osv(package).await;

        let mut cache = self.cache.lock().await;
        cache.insert(cache_key, CveFeedCacheEntry::new(verdict.clone(), self.clock.now()));
        verdict
    }
}

#[cfg(test)]
mod tests;
