//! Per-request decisions on traffic the proxy sees as HTTP: plain requests
//! relayed for an explicit proxy client and decrypted requests of a
//! terminated TLS connection. The destination was already admitted by the
//! policy; a [`RequestFilter`] then allows or denies the one request by its
//! method, path and headers.
//!
//! The declarative [`PolicyRequestFilter`] evaluates the `request_rules` of a
//! [`NetworkPolicy`]. The trait is the seam a callback supplied by the
//! embedding process plugs into without touching the relays.

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::{HttpMethod, NetworkPolicy, RefusedRequest};
use trogon_sandbox_contract::{RequestRule, RequestRuleAction};
use trogon_sandbox_protocol::{RequestSummary, evaluate_request_rules};

use crate::header_mutator::HeaderMutator;
use crate::header_syntax::fold_header_name;
use crate::proxy_headers::without_proxy_headers;
use crate::request_body::RequestBody;
use crate::request_decision::{RequestAllow, RequestDenial};
use crate::violation::ViolationReporter;

mod constants;
use constants::HOP_BY_HOP_HEADERS;

/// The headers a request carries once its hop-by-hop headers, and every
/// header its `Connection` header names, are gone, the
/// ones that frame the body or name the host included. The caller puts back
/// the framing and the `Host` the proxy does. Names match in any spelling of `-`,
/// `_` and `.`, as a folded match does.
pub(crate) fn without_hop_by_hop(headers: &[(String, String)]) -> Cow<'_, [(String, String)]> {
    let named: Vec<String> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .map(|token| fold_header_name(token.trim()))
        .collect();
    let dropped = |name: &str| {
        let folded = fold_header_name(name);
        HOP_BY_HOP_HEADERS.contains(&folded.as_str()) || named.contains(&folded)
    };
    if headers.iter().any(|(name, _)| dropped(name)) {
        Cow::Owned(headers.iter().filter(|(name, _)| !dropped(name)).cloned().collect())
    } else {
        Cow::Borrowed(headers)
    }
}

/// Whether a request arrived inside a terminated TLS tunnel or as plain HTTP.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RequestScheme {
    Http,
    Https,
}

impl RequestScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// The request-target in the forms a request callback receives. `spelled` is the
/// path and query as the client wrote them, without a fragment and with every
/// byte outside ASCII percent-encoded; `normalized` is the same target with
/// dot segments and repeated or backward slashes resolved; `ambiguous` says
/// the two can name different resources.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TargetForms {
    pub spelled: String,
    pub normalized: String,
    pub ambiguous: bool,
}

/// One request as a filter sees it. `path` is the request-target without its
/// query, spelled as it will be forwarded, and `query` is the text after the
/// first `?`. `sni` is the server name the client sent in its TLS handshake.
#[derive(Debug, Clone, Copy)]
pub struct RequestView<'a> {
    pub host: &'a Host,
    pub port: u16,
    pub method: &'a HttpMethod,
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub sni: Option<&'a str>,
    pub headers: &'a [(String, String)],
    /// `None` when the proxy does not hold the body at the time it decides.
    pub body: Option<RequestBody<'a>>,
    pub scheme: RequestScheme,
    /// `None` for a request whose target was not read in its two forms.
    pub forms: Option<&'a TargetForms>,
}

/// The verdict on one request. `Allow` and `Deny` are the plain verdicts;
/// the others carry the edits a request decision can.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestDecision {
    Allow,
    Deny,
    AllowWith(RequestAllow),
    DenyWith(RequestDenial),
}

impl From<bool> for RequestDecision {
    fn from(allowed: bool) -> Self {
        if allowed { Self::Allow } else { Self::Deny }
    }
}

impl RequestDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow | Self::AllowWith(_))
    }
}

#[async_trait]
pub trait RequestFilter: Send + Sync {
    async fn decide(&self, request: &RequestView<'_>) -> RequestDecision;
}

/// The `request_rules` of a policy: the first rule that matches decides, and
/// a request no rule matches is allowed.
#[derive(Debug, Clone)]
pub struct PolicyRequestFilter {
    rules: Vec<RequestRule>,
}

impl PolicyRequestFilter {
    pub fn from_policy(policy: &NetworkPolicy) -> Option<Self> {
        match policy {
            NetworkPolicy::UserDefined { request_rules, .. } if !request_rules.is_empty() => Some(Self {
                rules: request_rules.clone(),
            }),
            _ => None,
        }
    }
}

#[async_trait]
impl RequestFilter for PolicyRequestFilter {
    async fn decide(&self, request: &RequestView<'_>) -> RequestDecision {
        let summary = RequestSummary {
            host: request.host,
            port: request.port,
            method: request.method,
            path: request.path,
            headers: request.headers,
        };
        match evaluate_request_rules(&self.rules, &summary) {
            RequestRuleAction::Allow => RequestDecision::Allow,
            RequestRuleAction::Deny => RequestDecision::Deny,
        }
    }
}

/// A filter that asks each of `filters` in turn: the first denial wins and
/// the header edits and response hooks of the allows are merged.
pub fn all_of(filters: Vec<Arc<dyn RequestFilter>>) -> Arc<dyn RequestFilter> {
    Arc::new(AllOf(filters))
}

struct AllOf(Vec<Arc<dyn RequestFilter>>);

#[async_trait]
impl RequestFilter for AllOf {
    async fn decide(&self, request: &RequestView<'_>) -> RequestDecision {
        let mut allowed = RequestAllow::new();
        for filter in &self.0 {
            match filter.decide(request).await {
                RequestDecision::Allow => {}
                RequestDecision::AllowWith(allow) => allowed.merge(allow),
                denial => return denial,
            }
        }
        if allowed.is_empty() {
            RequestDecision::Allow
        } else {
            RequestDecision::AllowWith(allowed)
        }
    }
}

/// Told about each request a filter denied on a terminated connection, so it
/// can be recorded as a violation.
#[async_trait]
pub trait DeniedRequestObserver: Send + Sync {
    async fn request_denied(&self, host: &str, port: u16, request: RefusedRequest);
}

#[async_trait]
impl DeniedRequestObserver for ViolationReporter {
    async fn request_denied(&self, host: &str, port: u16, request: RefusedRequest) {
        self.report_request_denied(host, port, request).await;
    }
}

/// The filter that applies to a connection and where its denials are
/// reported.
#[derive(Clone)]
pub struct RequestChecks {
    filter: Option<Arc<dyn RequestFilter>>,
    observer: Option<Arc<dyn DeniedRequestObserver>>,
    plaintext_header_set: bool,
    mutator: Option<Arc<dyn HeaderMutator>>,
    plaintext_mutator: Option<Arc<dyn HeaderMutator>>,
}

impl std::fmt::Debug for RequestChecks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestChecks").finish_non_exhaustive()
    }
}

impl RequestChecks {
    /// The checks a connection under `policy` runs: the policy's own rules
    /// and the filter the embedding process supplied, if any. `None` when
    /// neither exists, so connections nothing applies to are left alone.
    pub(crate) fn assembled(
        policy: &NetworkPolicy,
        custom: Option<&Arc<dyn RequestFilter>>,
        observer: Option<&Arc<dyn DeniedRequestObserver>>,
        plaintext_header_set: bool,
        header_mutator: Option<&Arc<dyn HeaderMutator>>,
        plaintext_header_mutator: Option<&Arc<dyn HeaderMutator>>,
    ) -> Option<Self> {
        let mut filters: Vec<Arc<dyn RequestFilter>> = Vec::new();
        if let Some(rules) = PolicyRequestFilter::from_policy(policy) {
            filters.push(Arc::new(rules));
        }
        if let Some(custom) = custom {
            filters.push(Arc::clone(custom));
        }
        let filter: Option<Arc<dyn RequestFilter>> = match filters.len() {
            0 => None,
            1 => Some(filters.remove(0)),
            _ => Some(Arc::new(AllOf(filters))),
        };
        if filter.is_none() && header_mutator.is_none() && plaintext_header_mutator.is_none() {
            return None;
        }
        Some(Self {
            filter,
            observer: observer.cloned(),
            plaintext_header_set,
            mutator: header_mutator.cloned(),
            plaintext_mutator: plaintext_header_mutator.cloned(),
        })
    }

    pub fn new(filter: Arc<dyn RequestFilter>) -> Self {
        Self {
            filter: Some(filter),
            observer: None,
            plaintext_header_set: false,
            mutator: None,
            plaintext_mutator: None,
        }
    }

    /// Lets an allow set headers on a request that arrived in cleartext.
    /// Without it such an allow is refused rather than forwarded without the
    /// headers it relied on.
    pub fn with_plaintext_header_set(mut self, enabled: bool) -> Self {
        self.plaintext_header_set = enabled;
        self
    }

    pub(crate) fn plaintext_header_set(&self) -> bool {
        self.plaintext_header_set
    }

    /// Edits the headers of every terminated request just before it goes
    /// upstream.
    pub fn with_header_mutator(mut self, mutator: Arc<dyn HeaderMutator>) -> Self {
        self.mutator = Some(mutator);
        self
    }

    /// The same for requests that arrive in cleartext, off unless set.
    pub fn with_plaintext_header_mutator(mut self, mutator: Arc<dyn HeaderMutator>) -> Self {
        self.plaintext_mutator = Some(mutator);
        self
    }

    pub(crate) fn mutator(&self) -> Option<&Arc<dyn HeaderMutator>> {
        self.mutator.as_ref()
    }

    pub(crate) fn plaintext_mutator(&self) -> Option<&Arc<dyn HeaderMutator>> {
        self.plaintext_mutator.as_ref()
    }

    pub fn with_observer(mut self, observer: Arc<dyn DeniedRequestObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    pub(crate) async fn decide(&self, request: &RequestView<'_>) -> RequestDecision {
        match &self.filter {
            Some(filter) => {
                let headers = without_proxy_headers(request.headers);
                filter
                    .decide(&RequestView {
                        headers: &headers,
                        ..*request
                    })
                    .await
            }
            None => RequestDecision::Allow,
        }
    }

    pub(crate) async fn report_denied(&self, host: &str, port: u16, request: RefusedRequest) {
        if let Some(observer) = &self.observer {
            observer.request_denied(host, port, request).await;
        }
    }
}

#[cfg(test)]
mod tests;
