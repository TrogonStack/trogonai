//! Asks an external decider about each HTTP request to an admitted
//! destination, with the limits and fail-closed rules of the request decider protocol
//! protocol. The question travels as NATS request/reply
//! ([`RequestDecideRequest`]) instead of a framed stream; a request body is
//! shown on a second ask after the decider answers `need_body`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use trogon_sandbox_contract::{
    Base64Data, BodyTruncation, LeaseId, RequestDecideBody, RequestDecideRequest, RequestDecideResponse,
};
use trogon_sandbox_protocol::subjects::egress_request_decide;

use crate::decider::{DeciderLimits, DeciderTransport};
use crate::request_decision::{HeaderEdits, RequestAllow, RequestDenial, fold_header_name};
use crate::request_filter::{RequestDecision, RequestFilter, RequestView};

mod constants;
pub(crate) use constants::{
    CREDENTIAL_HEADERS, FRAMING_HEADERS, MAX_CREDENTIAL, MAX_HEADER_BYTES, MAX_HEADER_ENTRIES, MAX_HOST_PATH,
    MAX_REASON_BYTES, MAX_REMOVE_NAMES,
};
pub use constants::{MAX_REPLY_FRAME, MAX_REQUEST_BODY};

/// How a credential is put into the `Authorization` header.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CredentialScheme {
    Bearer,
    Basic { user: String },
}

/// Where a credential class's value goes. The first placement whose class
/// and host (when given) match wins; with none, the value goes as
/// `Authorization: Bearer <value>`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CredentialPlacement {
    pub class: Option<String>,
    pub host: Option<String>,
    pub scheme: CredentialScheme,
}

impl CredentialPlacement {
    /// The default for a git host: Basic with the user `x-access-token`.
    pub fn github() -> Self {
        Self {
            class: Some("github".to_string()),
            host: Some("github.com".to_string()),
            scheme: CredentialScheme::Basic {
                user: "x-access-token".to_string(),
            },
        }
    }

    fn matches(&self, class: &str, host: &str) -> bool {
        self.class.as_deref().is_none_or(|known| known == class)
            && self.host.as_deref().is_none_or(|known| known == host)
    }
}

pub(crate) fn unavailable(reason: &str) -> RequestDecision {
    RequestDecision::DenyWith(
        RequestDenial::new()
            .status(503)
            .reason(reason)
            .marked("decider_unavailable"),
    )
}

pub(crate) fn bad_request(reason: &str) -> RequestDecision {
    RequestDecision::DenyWith(RequestDenial::new().status(400).reason(reason).marked("bad_request"))
}

/// The parts of the decider protocol that do not depend on how a question
/// travels: what may be asked and how a verdict becomes a decision.
#[derive(Debug, Clone, Default)]
pub(crate) struct DeciderRules {
    placements: Vec<CredentialPlacement>,
}

impl DeciderRules {
    pub(crate) fn with_placements(placements: Vec<CredentialPlacement>) -> Self {
        Self { placements }
    }

    pub(crate) fn question(
        &self,
        lease: &LeaseId,
        request: &RequestView<'_>,
    ) -> Result<RequestDecideRequest, &'static str> {
        let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut host_header = None;
        let mut host_headers = 0;
        let (mut entries, mut bytes) = (0, 0);
        for (name, value) in request.headers {
            let name = name.to_ascii_lowercase();
            if !is_token(&name) || !is_value(value) {
                return Err("request header the decider cannot be asked about");
            }
            if name == "host" {
                host_headers += 1;
                host_header = Some(value.clone());
                continue;
            }
            entries += 1;
            bytes += value.len();
            let values = headers.entry(name.clone()).or_default();
            if values.is_empty() {
                bytes += name.len();
            }
            values.push(value.clone());
        }
        if host_headers > 1 {
            return Err("more than one Host header");
        }
        if entries > MAX_HEADER_ENTRIES || bytes > MAX_HEADER_BYTES {
            return Err("request headers too large");
        }
        let host = request.host.to_string();
        let (path, query) = match request.forms {
            Some(forms) => match forms.spelled.split_once('?') {
                Some((path, query)) => (path, Some(query)),
                None => (forms.spelled.as_str(), None),
            },
            None => (request.path, request.query),
        };
        if path.is_empty() || request.port == 0 {
            return Err("request the decider cannot be asked about");
        }
        for value in [
            host.as_str(),
            host_header.as_deref().unwrap_or(""),
            request.sni.unwrap_or(""),
            path,
        ] {
            if value.len() > MAX_HOST_PATH {
                return Err("request target too large");
            }
        }
        Ok(RequestDecideRequest {
            lease_id: lease.clone(),
            method: request.method.as_str().to_string(),
            host,
            port: request.port,
            host_header,
            sni: request.sni.filter(|sni| !sni.is_empty()).map(str::to_string),
            path: path.to_string(),
            query: query.filter(|query| !query.is_empty()).map(str::to_string),
            headers,
            body: None,
        })
    }

    pub(crate) fn placed(&self, class: &str, value: &str, host: &str) -> String {
        match self.placements.iter().find(|p| p.matches(class, host)) {
            Some(CredentialPlacement {
                scheme: CredentialScheme::Basic { user },
                ..
            }) => format!("Basic {}", STANDARD.encode(format!("{user}:{value}"))),
            _ => format!("Bearer {value}"),
        }
    }

    pub(crate) fn decision(&self, reply: RequestDecideResponse, host: &str) -> RequestDecision {
        match reply {
            RequestDecideResponse::Deny { status, reason } => {
                let reason_ok = reason
                    .as_deref()
                    .is_none_or(|r| !r.is_empty() && r.len() <= MAX_REASON_BYTES && is_value(r));
                if !(400..=599).contains(&status) || !reason_ok {
                    return unavailable("request decider: a malformed verdict");
                }
                let mut denial = RequestDenial::new().status(status).marked("decider");
                if let Some(reason) = reason {
                    denial = denial.reason(reason);
                }
                RequestDecision::DenyWith(denial)
            }
            RequestDecideResponse::Allow {
                set_headers,
                remove_headers,
                cred,
                credential,
            } => match self.edits(set_headers, remove_headers, cred, credential, host) {
                Some(edits) if edits.is_empty() => RequestDecision::Allow,
                Some(edits) => RequestDecision::AllowWith(RequestAllow::new().edits(edits)),
                None => unavailable("request decider: a malformed verdict"),
            },
            RequestDecideResponse::NeedBody { .. } => unavailable("request decider: a second need_body"),
        }
    }

    pub(crate) fn edits(
        &self,
        set_headers: BTreeMap<String, Vec<String>>,
        remove_headers: Vec<String>,
        cred: Option<String>,
        credential: Option<String>,
        host: &str,
    ) -> Option<HeaderEdits> {
        if cred.is_some() != credential.is_some() {
            return None;
        }
        let entries: usize = set_headers.values().map(Vec::len).sum();
        let bytes: usize = set_headers
            .iter()
            .map(|(name, values)| values.iter().map(|v| name.len() + v.len()).sum::<usize>())
            .sum();
        if entries > MAX_HEADER_ENTRIES
            || bytes > MAX_HEADER_BYTES
            || remove_headers.len() > MAX_REMOVE_NAMES
            || remove_headers.iter().map(String::len).sum::<usize>() > MAX_HEADER_BYTES
        {
            return None;
        }
        let mut seen = std::collections::BTreeSet::new();
        if !remove_headers.iter().all(|name| seen.insert(name.as_str())) {
            return None;
        }
        let never_set = |name: &str| {
            let folded = fold_header_name(name);
            CREDENTIAL_HEADERS.contains(&folded.as_str()) || FRAMING_HEADERS.contains(&folded.as_str())
        };
        let mut edits = HeaderEdits::new();
        for (name, values) in &set_headers {
            if values.is_empty() || !is_token(name) || name != &name.to_ascii_lowercase() || never_set(name) {
                return None;
            }
            for value in values {
                edits = edits.set(name.clone(), value.clone()).ok()?;
            }
        }
        for name in &remove_headers {
            let folded = fold_header_name(name);
            if !is_token(name) || name != &name.to_ascii_lowercase() || FRAMING_HEADERS.contains(&folded.as_str()) {
                return None;
            }
            edits = edits.remove(name.clone()).ok()?;
        }
        if let (Some(class), Some(value)) = (cred, credential) {
            if class.is_empty()
                || value.is_empty()
                || value.len() > MAX_CREDENTIAL
                || !value.bytes().all(|b| (0x21..=0x7e).contains(&b))
            {
                return None;
            }
            for name in CREDENTIAL_HEADERS {
                edits = edits.remove(name).ok()?;
            }
            edits = edits.set("authorization", self.placed(&class, &value, host)).ok()?;
        }
        Some(edits)
    }
}

pub struct RequestDecider {
    lease: LeaseId,
    transport: Arc<dyn DeciderTransport>,
    limits: DeciderLimits,
    rules: DeciderRules,
    permits: Arc<Semaphore>,
}

impl RequestDecider {
    pub fn new(lease: LeaseId, transport: Arc<dyn DeciderTransport>, limits: DeciderLimits) -> Self {
        Self {
            lease,
            transport,
            permits: Arc::new(Semaphore::new(limits.max_in_flight)),
            limits,
            rules: DeciderRules::default(),
        }
    }

    pub fn credential_placements(mut self, placements: Vec<CredentialPlacement>) -> Self {
        self.rules = DeciderRules { placements };
        self
    }

    async fn ask(&self, question: &RequestDecideRequest) -> Result<RequestDecideResponse, String> {
        let payload = serde_json::to_vec(question).map_err(|err| err.to_string())?;
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| "the decider is closed".to_string())?;
        let reply = timeout(
            self.limits.timeout,
            self.transport.request(egress_request_decide(&self.lease), payload),
        )
        .await
        .map_err(|_| "the decider did not answer in time".to_string())?
        .map_err(|err| err.to_string())?;
        if reply.len() > MAX_REPLY_FRAME {
            return Err("the decider reply is too large".to_string());
        }
        serde_json::from_slice(&reply).map_err(|_| "the decider reply is malformed".to_string())
    }
}

#[async_trait]
impl RequestFilter for RequestDecider {
    async fn decide(&self, request: &RequestView<'_>) -> RequestDecision {
        let mut question = match self.rules.question(&self.lease, request) {
            Ok(question) => question,
            Err(reason) => return bad_request(reason),
        };
        let host = question.host.clone();
        let first = match self.ask(&question).await {
            Ok(reply) => reply,
            Err(err) => return unavailable(&format!("request decider: {err}")),
        };
        let RequestDecideResponse::NeedBody { max } = first else {
            return self.rules.decision(first, &host);
        };
        if !(1..=MAX_REQUEST_BODY).contains(&max) {
            return unavailable("request decider: a malformed verdict");
        }
        let Some(body) = request.body else {
            return unavailable("request decider: the request body is not available");
        };
        let wanted = usize::try_from(max).unwrap_or(usize::MAX);
        let Ok(read) = body.read(wanted).await else {
            return unavailable("request decider: the request body is not available");
        };
        question.body = Base64Data::new(STANDARD.encode(&read.bytes))
            .ok()
            .map(|data| RequestDecideBody {
                data,
                cut: BodyTruncation::from(read.cut),
            });
        match self.ask(&question).await {
            Ok(reply) => self.rules.decision(reply, &host),
            Err(err) => unavailable(&format!("request decider: {err}")),
        }
    }
}

pub(crate) fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

pub(crate) fn is_value(value: &str) -> bool {
    !value
        .chars()
        .any(|c| (c < ' ' && c != '\t') || ('\u{7f}'..='\u{9f}').contains(&c))
}

#[cfg(test)]
mod tests;
