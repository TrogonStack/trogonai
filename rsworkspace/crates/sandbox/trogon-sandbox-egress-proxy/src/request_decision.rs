//! What a request filter can say about one request beyond allow or deny: the
//! parts of a request decision a Rust caller can use. An allow can edit
//! the headers that are forwarded and observe the response; a deny can pick
//! the status, a reason and a mark.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::SystemTime;

use crate::framed_head::{AnsweredRequest, FramedAnswer};
pub use crate::header_syntax::fold_header_name;
use crate::header_syntax::{is_header_value, is_token};

mod constants;
pub use constants::DEFAULT_DENY_REASON;
use constants::UNSETTABLE_HEADERS;

/// A decision that cannot be applied as given. The proxy treats the request
/// as denied rather than forwarding it half-edited.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum MalformedDecisionError {
    #[error("{0:?} is not an HTTP header name")]
    HeaderName(String),
    #[error("a header value holds a control character other than tab, or a character above U+00FF")]
    HeaderValue,
    #[error("{0:?} is not a reason code")]
    Mark(String),
}

/// Header removals and sets of an allow. Names match case-insensitively with
/// `-`, `_` and `.` read as one character. Framing headers are never removed
/// or set, and a set first removes every spelling of its own name.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct HeaderEdits {
    remove: Vec<String>,
    set: Vec<(String, String)>,
}

impl HeaderEdits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remove(mut self, name: impl Into<String>) -> Result<Self, MalformedDecisionError> {
        let name = name.into();
        if !is_token(&name) {
            return Err(MalformedDecisionError::HeaderName(name));
        }
        self.remove.push(name);
        Ok(self)
    }

    /// A name given more than once gets every value, in order.
    pub fn set(mut self, name: impl Into<String>, value: impl Into<String>) -> Result<Self, MalformedDecisionError> {
        let (name, value) = (name.into(), value.into());
        if !is_token(&name) {
            return Err(MalformedDecisionError::HeaderName(name));
        }
        if !is_header_value(&value) {
            return Err(MalformedDecisionError::HeaderValue);
        }
        self.set.push((name, value));
        Ok(self)
    }

    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.set.is_empty()
    }

    pub fn sets_headers(&self) -> bool {
        !self.set.is_empty()
    }

    pub fn merge(&mut self, other: HeaderEdits) {
        self.remove.extend(other.remove);
        self.set.extend(other.set);
    }

    /// Removes first, then adds the sets after the headers that remain.
    pub fn apply(&self, headers: &mut Vec<(String, String)>) {
        let mut sets: Vec<(String, String, Vec<&str>)> = Vec::new();
        for (name, value) in &self.set {
            let folded = fold_header_name(name);
            if UNSETTABLE_HEADERS.contains(&folded.as_str()) {
                continue;
            }
            match sets.iter_mut().find(|(known, _, _)| *known == folded) {
                Some((_, _, values)) => values.push(value),
                None => sets.push((folded, name.to_ascii_lowercase(), vec![value])),
            }
        }
        let mut drop: Vec<String> = self
            .remove
            .iter()
            .map(|name| fold_header_name(name))
            .filter(|folded| !UNSETTABLE_HEADERS.contains(&folded.as_str()))
            .collect();
        drop.extend(sets.iter().map(|(folded, _, _)| folded.clone()));
        headers.retain(|(name, _)| !drop.contains(&fold_header_name(name)));
        for (_, name, values) in sets {
            for value in values {
                headers.push((name.clone(), value.to_string()));
            }
        }
    }
}

type ResponseHookFn = dyn Fn(u16, &[(String, String)]) + Send + Sync;

/// Called once with the status and headers of the upstream response, just
/// before it is written to the client. It sees a copy, so it cannot change
/// the response, and a panic is contained.
#[derive(Clone)]
pub struct ResponseHook(Arc<ResponseHookFn>);

impl ResponseHook {
    pub fn new(hook: impl Fn(u16, &[(String, String)]) + Send + Sync + 'static) -> Self {
        Self(Arc::new(hook))
    }

    pub(crate) fn notify(&self, status: u16, headers: &[(String, String)]) {
        let copy = headers.to_vec();
        if catch_unwind(AssertUnwindSafe(|| (self.0)(status, &copy))).is_err() {
            tracing::error!("a response hook panicked");
        }
    }

    pub(crate) fn then(self, next: ResponseHook) -> ResponseHook {
        ResponseHook(Arc::new(move |status, headers| {
            self.notify(status, headers);
            next.notify(status, headers);
        }))
    }
}

impl std::fmt::Debug for ResponseHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResponseHook")
    }
}

impl PartialEq for ResponseHook {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ResponseHook {}

/// What an allow does to the request it lets through and to its response.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct RequestAllow {
    edits: HeaderEdits,
    on_response: Option<ResponseHook>,
}

impl RequestAllow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn edits(mut self, edits: HeaderEdits) -> Self {
        self.edits = edits;
        self
    }

    pub fn on_response(mut self, hook: ResponseHook) -> Self {
        self.on_response = Some(hook);
        self
    }

    pub fn header_edits(&self) -> &HeaderEdits {
        &self.edits
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty() && self.on_response.is_none()
    }

    pub(crate) fn response_hook(&self) -> Option<&ResponseHook> {
        self.on_response.as_ref()
    }

    pub(crate) fn merge(&mut self, other: RequestAllow) {
        self.edits.merge(other.edits);
        self.on_response = match (self.on_response.take(), other.on_response) {
            (Some(first), Some(second)) => Some(first.then(second)),
            (first, second) => first.or(second),
        };
    }
}

/// A short reason code for a refusal: an HTTP token such as `decider`. It is
/// the value of the `X-Proxy-Error` header of the answer.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DenyMark(String);

impl DenyMark {
    pub fn parse(mark: impl Into<String>) -> Result<Self, MalformedDecisionError> {
        let mark = mark.into();
        if is_token(&mark) {
            Ok(Self(mark))
        } else {
            Err(MalformedDecisionError::Mark(mark))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How a deny is answered: a status from 400 to 599 (anything else answers
/// 403), a reason that becomes the body, and a mark.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct RequestDenial {
    reason: Option<String>,
    status: Option<u16>,
    mark: Option<DenyMark>,
}

impl RequestDenial {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn plaintext_header_set() -> Self {
        Self {
            reason: Some(
                "the allow decision sets headers, and headers are not set on plain-HTTP requests unless plaintext header sets are enabled; nothing was forwarded"
                    .to_string(),
            ),
            status: None,
            mark: DenyMark::parse("plaintext_header_set").ok(),
        }
    }

    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub(crate) fn marked(mut self, mark: &str) -> Self {
        self.mark = DenyMark::parse(mark).ok().or(self.mark.take());
        self
    }

    pub fn mark(mut self, mark: DenyMark) -> Self {
        self.mark = Some(mark);
        self
    }

    pub fn denial_reason(&self) -> &str {
        self.reason.as_deref().unwrap_or(DEFAULT_DENY_REASON)
    }

    pub fn answer_status(&self) -> u16 {
        match self.status {
            Some(status) if (400..=599).contains(&status) => status,
            _ => 403,
        }
    }

    /// The answer written for this denial at `at`
    /// through a strict HTTP server.
    pub(crate) fn answer(&self, request: AnsweredRequest, at: SystemTime) -> Vec<u8> {
        FramedAnswer::plain(
            self.answer_status(),
            crate::deny_header_style::denial_field(self.mark.as_ref().map(DenyMark::as_str)),
            format!("{}\n", self.denial_reason()),
        )
        .bytes(request, at)
    }
}

#[cfg(test)]
mod tests;
