//! An in-memory record of what a sandbox refused, for a process that embeds
//! the sandbox and reads violations back instead of receiving them on a
//! message bus.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{AddressDenialClass, EgressViolation, EgressViolationReason, UnsupportedSigv4Shape};

use crate::violation::{ViolationPublishError, ViolationSink};

mod constants;
use constants::MAX_REMEMBERED_REASONS;
pub use constants::{DEFAULT_DENIAL_REASON, MAX_DENIAL_REASON_CHARS, MAX_RETAINED_VIOLATIONS};

/// Names the command a violation belongs to.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct CommandId(String);

impl CommandId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_display_unsafe(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{1f}'
        | '\u{7f}'..='\u{9f}'
        | '\u{ad}'
        | '\u{61c}'
        | '\u{200b}'..='\u{200f}'
        | '\u{2028}'
        | '\u{2029}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{feff}'
        | '\u{e0000}'..='\u{e007f}')
}

/// Text safe to show next to other text: a run of control, bidirectional or
/// zero-width characters collapses to one space, and the angle brackets that
/// would close an enclosing `<sandbox_violations>` envelope are dropped.
fn sanitize_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_unsafe_run = false;
    for c in text.chars() {
        if is_display_unsafe(c) {
            if !in_unsafe_run {
                out.push(' ');
            }
            in_unsafe_run = true;
            continue;
        }
        in_unsafe_run = false;
        if c != '<' && c != '>' {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// A line of violation text as the store keeps it.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ViolationLine(String);

impl ViolationLine {
    pub fn new(text: &str) -> Self {
        Self(sanitize_text(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ViolationLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a destination was refused, as an ask callback gave it: sanitized and
/// cut to [`MAX_DENIAL_REASON_CHARS`] characters.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DenialReason(String);

impl DenialReason {
    pub fn new(reason: &str) -> Self {
        let sanitized = sanitize_text(reason);
        let cut: String = sanitized.chars().take(MAX_DENIAL_REASON_CHARS).collect();
        match cut.trim_end() {
            "" => Self::default(),
            reason => Self(reason.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for DenialReason {
    fn default() -> Self {
        Self(DEFAULT_DENIAL_REASON.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct SandboxViolation {
    pub line: ViolationLine,
    pub command: Option<CommandId>,
    pub at: SystemTime,
}

type Listener = Arc<dyn Fn(&[SandboxViolation]) + Send + Sync>;

struct Inner {
    violations: VecDeque<SandboxViolation>,
    total: u64,
    listeners: Vec<(u64, Listener)>,
    next_listener: u64,
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The last [`MAX_RETAINED_VIOLATIONS`] violations, newest last. The total
/// count of violations ever added survives [`ViolationStore::clear`].
#[derive(Clone)]
pub struct ViolationStore {
    inner: Arc<Mutex<Inner>>,
}

impl Default for ViolationStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Stops its listener when dropped or when [`Subscription::unsubscribe`] is
/// called.
pub struct Subscription {
    store: Arc<Mutex<Inner>>,
    id: u64,
}

impl Subscription {
    pub fn unsubscribe(self) {}
}

impl Drop for Subscription {
    fn drop(&mut self) {
        lock(&self.store).listeners.retain(|(id, _)| *id != self.id);
    }
}

impl ViolationStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                violations: VecDeque::new(),
                total: 0,
                listeners: Vec::new(),
                next_listener: 0,
            })),
        }
    }

    pub fn add(&self, line: &str, command: Option<CommandId>) {
        self.add_at(line, command, SystemTime::now());
    }

    pub fn add_at(&self, line: &str, command: Option<CommandId>, at: SystemTime) {
        let (snapshot, listeners) = {
            let mut inner = lock(&self.inner);
            inner.violations.push_back(SandboxViolation {
                line: ViolationLine::new(line),
                command,
                at,
            });
            inner.total += 1;
            while inner.violations.len() > MAX_RETAINED_VIOLATIONS {
                inner.violations.pop_front();
            }
            (Self::snapshot(&inner), Self::listeners(&inner))
        };
        notify(&listeners, &snapshot);
    }

    /// The most recent `limit` violations, or all retained ones.
    pub fn violations(&self, limit: Option<usize>) -> Vec<SandboxViolation> {
        let all = Self::snapshot(&lock(&self.inner));
        match limit {
            Some(limit) if limit < all.len() => all[all.len() - limit..].to_vec(),
            _ => all,
        }
    }

    pub fn violations_for_command(&self, command: &CommandId) -> Vec<SandboxViolation> {
        lock(&self.inner)
            .violations
            .iter()
            .filter(|violation| violation.command.as_ref() == Some(command))
            .cloned()
            .collect()
    }

    pub fn count(&self) -> usize {
        lock(&self.inner).violations.len()
    }

    pub fn total_count(&self) -> u64 {
        lock(&self.inner).total
    }

    pub fn clear(&self) {
        let listeners = {
            let mut inner = lock(&self.inner);
            inner.violations.clear();
            Self::listeners(&inner)
        };
        notify(&listeners, &[]);
    }

    /// Calls `listener` at once with the current violations, then on every
    /// change with all retained violations.
    pub fn subscribe(&self, listener: impl Fn(&[SandboxViolation]) + Send + Sync + 'static) -> Subscription {
        let listener: Listener = Arc::new(listener);
        let (id, snapshot) = {
            let mut inner = lock(&self.inner);
            let id = inner.next_listener;
            inner.next_listener += 1;
            inner.listeners.push((id, Arc::clone(&listener)));
            (id, Self::snapshot(&inner))
        };
        listener(&snapshot);
        Subscription {
            store: Arc::clone(&self.inner),
            id,
        }
    }

    fn snapshot(inner: &Inner) -> Vec<SandboxViolation> {
        inner.violations.iter().cloned().collect()
    }

    fn listeners(inner: &Inner) -> Vec<Listener> {
        inner
            .listeners
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect()
    }
}

fn notify(listeners: &[Listener], snapshot: &[SandboxViolation]) {
    for listener in listeners {
        listener(snapshot);
    }
}

/// Remembers why an ask callback refused a destination, so the violation the
/// proxy reports for that refusal can carry the callback's own words.
#[derive(Clone, Default)]
pub struct DenialReasons {
    reasons: Arc<Mutex<VecDeque<(String, u16, DenialReason)>>>,
}

impl DenialReasons {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remember(&self, host: &str, port: u16, reason: DenialReason) {
        let mut reasons = self.reasons.lock().unwrap_or_else(|p| p.into_inner());
        reasons.retain(|(h, p, _)| !(h == host && *p == port));
        reasons.push_back((host.to_string(), port, reason));
        while reasons.len() > MAX_REMEMBERED_REASONS {
            reasons.pop_front();
        }
    }

    fn take(&self, host: &str, port: u16) -> Option<DenialReason> {
        let mut reasons = self.reasons.lock().unwrap_or_else(|p| p.into_inner());
        let index = reasons.iter().position(|(h, p, _)| h == host && *p == port)?;
        reasons.remove(index).map(|(_, _, reason)| reason)
    }
}

/// Violation lines to leave out of the store, keyed by a fragment of the
/// command text: the `*` key applies to every command, any other key applies
/// to commands whose text contains it. A line is ignored when it contains one
/// of the patterns that apply to it.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct IgnoreViolations {
    rules: BTreeMap<String, Vec<String>>,
}

impl IgnoreViolations {
    pub const EVERY_COMMAND: &'static str = "*";

    pub fn new(rules: BTreeMap<String, Vec<String>>) -> Self {
        Self { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn ignores(&self, line: &str, command_text: Option<&str>) -> bool {
        let matches_any = |patterns: &[String]| patterns.iter().any(|p| line.contains(p.as_str()));
        if self
            .rules
            .get(Self::EVERY_COMMAND)
            .is_some_and(|patterns| matches_any(patterns))
        {
            return true;
        }
        let Some(command_text) = command_text else {
            return false;
        };
        self.rules.iter().any(|(fragment, patterns)| {
            fragment != Self::EVERY_COMMAND && command_text.contains(fragment.as_str()) && matches_any(patterns)
        })
    }
}

/// An [`IgnoreViolations`] bound to the text of one command.
#[derive(Debug, Clone, Default)]
pub struct ViolationFilter {
    ignore: Arc<IgnoreViolations>,
    command_text: Option<String>,
}

impl ViolationFilter {
    pub fn new(ignore: Arc<IgnoreViolations>, command_text: Option<String>) -> Self {
        Self { ignore, command_text }
    }

    pub fn ignores(&self, line: &str) -> bool {
        self.ignore.ignores(line, self.command_text.as_deref())
    }
}

/// Records the violations the proxy reports for one command into a
/// [`ViolationStore`].
pub struct StoreViolationSink {
    store: ViolationStore,
    command: CommandId,
    reasons: DenialReasons,
    filter: ViolationFilter,
}

impl StoreViolationSink {
    pub fn new(store: ViolationStore, command: CommandId, reasons: DenialReasons) -> Self {
        Self {
            store,
            command,
            reasons,
            filter: ViolationFilter::default(),
        }
    }

    pub fn with_filter(mut self, filter: ViolationFilter) -> Self {
        self.filter = filter;
        self
    }

    fn describe(&self, violation: &EgressViolation) -> String {
        if let Some(request) = &violation.request {
            if violation.reason == EgressViolationReason::OpaqueTunnelRefused {
                return format!("deny socks-tunnel {} ({})", request.target(), request.reason());
            }
            return format!(
                "deny http-request {} {} ({})",
                request.method(),
                request.target(),
                request.reason()
            );
        }
        let target = match (&violation.host, violation.dest_ip) {
            (Some(host), _) => format!("{host}:{}", violation.port),
            (None, Some(ip)) => format!("{ip}:{}", violation.port),
            (None, None) => format!("unknown:{}", violation.port),
        };
        let reason = match &violation.reason {
            EgressViolationReason::DeciderDenied => violation
                .host
                .as_deref()
                .and_then(|host| self.reasons.take(host, violation.port))
                .unwrap_or_default()
                .as_str()
                .to_string(),
            EgressViolationReason::PolicyDenied => violation
                .host
                .as_deref()
                .and_then(|host| self.reasons.take(host, violation.port))
                .map_or_else(
                    || "host is not on the allow list".to_string(),
                    |reason| reason.as_str().to_string(),
                ),
            EgressViolationReason::TunnelLimitReached => {
                return format!(
                    "deny http-request CONNECT {target} (the sandbox's limit of TLS-terminated tunnels at once is reached; the limit is network.tlsTerminate.maxTunnels in the sandbox's configuration)"
                );
            }
            EgressViolationReason::RequestDenied => "request refused by filter".to_string(),
            EgressViolationReason::DestinationAddressDenied(class) => violation
                .host
                .as_deref()
                .and_then(|host| self.reasons.take(host, violation.port))
                .map_or_else(|| resolved_to_words(&[*class]), |reason| reason.as_str().to_string()),
            EgressViolationReason::DeciderTimedOut
            | EgressViolationReason::DeciderUnavailable
            | EgressViolationReason::DeciderMalformedReply
            | EgressViolationReason::DeciderOverloaded => "permission prompt failed".to_string(),
            EgressViolationReason::OpaqueTunnelRefused => {
                return format!(
                    "deny http-request CONNECT {target} (this proxy does not tunnel CONNECTs it cannot inspect)"
                );
            }
            EgressViolationReason::MalformedHost => "malformed host".to_string(),
            EgressViolationReason::PolicyUnavailable => "sandbox policy unavailable".to_string(),
            EgressViolationReason::GssEncryptionUnsupported => {
                "the proxy cannot carry GSS-encrypted PostgreSQL sessions".to_string()
            }
            EgressViolationReason::PostgresTlsDeclined => {
                "the PostgreSQL server declined TLS, so a credential could not be substituted".to_string()
            }
            EgressViolationReason::SecretSentinelOffHost => {
                "a masked credential was sent to a host it is not bound to".to_string()
            }
            EgressViolationReason::SecretCompressedResponse => {
                "the response is compressed, so a credential could not be handled".to_string()
            }
            EgressViolationReason::SniDestinationMismatch => {
                "the TLS server name does not match the requested host".to_string()
            }
            EgressViolationReason::SniResolutionFailed => "the TLS server name could not be resolved".to_string(),
            EgressViolationReason::SecretSigv4Unsupported(shape) => format!(
                "a masked AWS credential cannot be re-signed for a {} request",
                match shape {
                    UnsupportedSigv4Shape::Streaming => "streaming-signed",
                    UnsupportedSigv4Shape::Presigned => "presigned",
                    UnsupportedSigv4Shape::Sigv4a => "SigV4A",
                }
            ),
            EgressViolationReason::SecretSigv4Unsignable => {
                "the request cannot be re-signed after substituting a masked AWS credential".to_string()
            }
            EgressViolationReason::SecretJwtMalformed => "a masked JWT credential is malformed".to_string(),
            EgressViolationReason::SecretJwtClaimsUnmatched => {
                "the claims of a masked JWT credential do not match this request".to_string()
            }
        };
        format!("deny network-outbound {target} ({reason})")
    }
}

pub(crate) fn address_class_words(class: AddressDenialClass) -> &'static str {
    match class {
        AddressDenialClass::DenyListed => "a deny-listed address",
        AddressDenialClass::Loopback => "a loopback address",
        AddressDenialClass::Unspecified => "an unspecified address",
        AddressDenialClass::CloudMetadata => "a cloud metadata address",
        AddressDenialClass::LinkLocal => "a link-local address",
        AddressDenialClass::Multicast => "a multicast address",
        AddressDenialClass::Broadcast => "the broadcast address",
        AddressDenialClass::OwnInterface => "one of this host's addresses",
        AddressDenialClass::PrivateUse | AddressDenialClass::Listed => "a listed address",
        AddressDenialClass::NonLoopback => "a non-loopback address",
    }
}

/// `resolved to <class> / <class>`: the classes in a fixed order,
/// however the resolver ordered its answers.
pub(crate) fn resolved_to_words(classes: &[AddressDenialClass]) -> String {
    let rank = |class: AddressDenialClass| match class {
        AddressDenialClass::DenyListed => 0,
        AddressDenialClass::Loopback => 1,
        AddressDenialClass::Unspecified => 2,
        AddressDenialClass::LinkLocal => 3,
        AddressDenialClass::Multicast => 4,
        AddressDenialClass::Broadcast => 5,
        AddressDenialClass::CloudMetadata => 6,
        AddressDenialClass::PrivateUse | AddressDenialClass::Listed => 7,
        AddressDenialClass::OwnInterface => 8,
        AddressDenialClass::NonLoopback => 9,
    };
    let mut ordered = classes.to_vec();
    ordered.sort_by_key(|class| rank(*class));
    let mut words: Vec<&str> = ordered.into_iter().map(address_class_words).collect();
    words.dedup();
    format!("resolved to {}", words.join(" / "))
}

#[async_trait]
impl ViolationSink for StoreViolationSink {
    async fn publish(&self, _subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        let violation: EgressViolation =
            serde_json::from_slice(&payload).map_err(|err| ViolationPublishError(err.to_string()))?;
        let line = self.describe(&violation);
        if self.filter.ignores(&line) {
            return Ok(());
        }
        self.store.add_at(&line, Some(self.command.clone()), violation.at);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
