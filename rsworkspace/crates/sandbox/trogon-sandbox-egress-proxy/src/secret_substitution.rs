//! Sentinel to real value substitution for TLS-terminated requests.
//!
//! A [`SecretSubstitution`] wraps the lease's [`SentinelRegistry`], whose
//! per-binding host gating decides which sentinels may be swapped for a given
//! destination. The destination is the TLS target (the host the client's
//! handshake named and the port dialed), never a client-supplied `Host`
//! header. Sentinels of bindings that do not inject at the destination are
//! left untouched and reported through a [`SentinelMisdirectionObserver`].
//!
//! Responses travel the other way. The same automaton shape is built from the
//! real values of every binding in the registry and swaps them back for their
//! sentinels in response headers and uncompressed bodies, so an allowed host
//! that reflects what it was sent cannot hand a secret to the sandbox. A
//! compressed response cannot be scanned, so a [`CompressedResponsePolicy`]
//! decides what happens to one from a host a binding injects at.
//!
//! Substitution is fake to real, so every miss (a compressed body, an encoded
//! sentinel) lets the useless fake through and can never leak a secret. All
//! sentinels are scanned with one Aho-Corasick automaton, holding back the
//! last `longest sentinel - 1` bytes of each chunk so a sentinel split across
//! chunk boundaries is still seen whole. Real values never reach a log, an
//! error or a violation.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use aho_corasick::{AhoCorasick, MatchKind};
use async_trait::async_trait;
use trogon_sandbox_contract::EgressViolationReason;
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::ResponseHeaderStrip;
use trogon_sandbox_contract::SecretName;
use trogon_sandbox_protocol::{RealValue, RequestSite, Sentinel, SentinelRegistry};

use crate::sigv4_resign::{AwsPairs, PairRefusal, ResignPlan, Sigv4Policies};
use crate::tls_target::TlsTarget;
use crate::violation::ViolationReporter;

mod constants;
pub(crate) use constants::MIN_SCRUBBED_VALUE_LEN;

#[async_trait]
pub trait SentinelMisdirectionObserver: Send + Sync {
    async fn sentinel_off_host(&self, host: &str, port: u16, binding: &SecretName);

    async fn compressed_response(&self, host: &str, port: u16, binding: &SecretName);

    async fn sigv4_refused(&self, _host: &str, _port: u16, _binding: &SecretName, _reason: EgressViolationReason) {}
}

#[async_trait]
impl SentinelMisdirectionObserver for ViolationReporter {
    async fn sentinel_off_host(&self, host: &str, port: u16, binding: &SecretName) {
        self.report_sentinel_off_host(host, port, binding).await;
    }

    async fn compressed_response(&self, host: &str, port: u16, binding: &SecretName) {
        self.report_compressed_response(host, port, binding).await;
    }

    async fn sigv4_refused(&self, host: &str, port: u16, binding: &SecretName, reason: EgressViolationReason) {
        self.report_sigv4_refused(host, port, binding, reason).await;
    }
}

/// What happens to a compressed response from a host a binding injects at.
/// Its body cannot be scanned for real values, so the default refuses it
/// rather than let one through unseen. `Flag` forwards it unscanned and
/// reports a violation naming each binding, for hosts where that is a
/// deliberate trade.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CompressedResponsePolicy {
    #[default]
    Refuse,
    Flag,
}

#[derive(Clone)]
pub struct SecretSubstitution {
    registry: Arc<SentinelRegistry>,
    observer: Option<Arc<dyn SentinelMisdirectionObserver>>,
    compressed_response: CompressedResponsePolicy,
    sigv4: Sigv4Policies,
}

impl SecretSubstitution {
    pub fn new(registry: Arc<SentinelRegistry>) -> Self {
        Self {
            registry,
            observer: None,
            compressed_response: CompressedResponsePolicy::default(),
            sigv4: Sigv4Policies::default(),
        }
    }

    pub fn with_sigv4_policies(mut self, policies: Sigv4Policies) -> Self {
        self.sigv4 = policies;
        self
    }

    pub fn with_compressed_response_policy(mut self, policy: CompressedResponsePolicy) -> Self {
        self.compressed_response = policy;
        self
    }

    pub fn with_observer(mut self, observer: Arc<dyn SentinelMisdirectionObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// True when some binding injects at `host:port`, so the connection has
    /// to be TLS-terminated for substitution to happen at all.
    pub fn covers(&self, host: &str, port: u16) -> bool {
        Host::new(host)
            .map(|host| !self.registry.names_injectable_at(&host, port).is_empty())
            .unwrap_or(false)
    }

    /// The connection-level scope. A binding scoped to a path or method is
    /// only flagged here, never replaced: [`SubstitutionScope::for_request`]
    /// decides it once a request is read, so a caller that skips that step
    /// fails closed.
    pub(crate) fn scope_for(&self, target: &TlsTarget) -> Option<SubstitutionScope> {
        self.scope_with(target, None)
    }

    fn scope_with(&self, target: &TlsTarget, request: Option<(&str, &str)>) -> Option<SubstitutionScope> {
        let host = Host::new(target.host()).ok()?;
        let site = request.map(|(method, path)| RequestSite { method, path });
        let mut forward = Vec::new();
        for (sentinel, real) in self.registry.pairs_for_request(&host, target.port(), site) {
            forward.push((sentinel.as_str().as_bytes(), Action::Replace(real.clone())));
        }
        for (name, sentinel) in self.registry.pairs_off_request(&host, target.port(), site) {
            forward.push((sentinel.as_str().as_bytes(), Action::Flag(name.clone())));
        }
        let request = Rules::build(forward)?;
        let reverse = self
            .registry
            .pairs()
            .filter(|(_, real)| real.expose().len() >= MIN_SCRUBBED_VALUE_LEN)
            .map(|(sentinel, real)| (real.expose().as_bytes(), Action::Scrub(sentinel.clone())))
            .collect();
        let injecting = self
            .registry
            .names_injectable_for(&host, target.port(), site)
            .into_iter()
            .cloned()
            .collect();
        Some(SubstitutionScope {
            request,
            response: Rules::build(reverse),
            strip: self.registry.strip_for_host(&host, target.port()),
            injecting,
            aws: AwsPairs::for_target(&self.registry, &host, target.port(), site, self.sigv4),
            compressed_response: self.compressed_response,
            target: target.clone(),
            observer: self.observer.clone(),
            owner: Some(self.clone()),
        })
    }
}

impl fmt::Debug for SecretSubstitution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretSubstitution")
            .field("bindings", &self.registry.len())
            .finish()
    }
}

#[derive(Clone)]
enum Action {
    Replace(RealValue),
    Flag(SecretName),
    Scrub(Sentinel),
}

/// One Aho-Corasick automaton and what to do on each of its patterns.
#[derive(Clone)]
struct Rules {
    automaton: Arc<AhoCorasick>,
    actions: Arc<[Action]>,
    max_len: usize,
    length_matched: bool,
}

impl Rules {
    fn build(entries: Vec<(&[u8], Action)>) -> Option<Self> {
        let max_len = entries.iter().map(|(pattern, _)| pattern.len()).max()?;
        let length_matched = entries.iter().all(|(pattern, action)| match action {
            Action::Replace(real) => real.expose().len() == pattern.len(),
            Action::Flag(_) | Action::Scrub(_) => true,
        });
        let automaton = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(entries.iter().map(|(pattern, _)| pattern))
            .ok()?;
        Some(Self {
            automaton: Arc::new(automaton),
            actions: entries.into_iter().map(|(_, action)| action).collect(),
            max_len,
            length_matched,
        })
    }
}

/// The sentinels that matter for one TLS target: those whose binding injects
/// there are replaced, all others are only reported. Responses are scrubbed of
/// the real value of every binding.
#[derive(Clone)]
pub struct SubstitutionScope {
    request: Rules,
    response: Option<Rules>,
    strip: ResponseHeaderStrip,
    injecting: Arc<[SecretName]>,
    aws: AwsPairs,
    compressed_response: CompressedResponsePolicy,
    target: TlsTarget,
    observer: Option<Arc<dyn SentinelMisdirectionObserver>>,
    owner: Option<SecretSubstitution>,
}

impl fmt::Debug for SubstitutionScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubstitutionScope")
            .field("target", &self.target)
            .field("sentinels", &self.request.actions.len())
            .finish()
    }
}

impl SubstitutionScope {
    /// The scope for one request: bindings scoped to a path prefix or a
    /// method are replaced only when `method` and `path` (the request target
    /// without its query) are in their scope. Out-of-scope bindings keep
    /// their sentinel, which is reported like one sent to the wrong host. Call
    /// it after the request's `Host` and target have been validated.
    pub(crate) fn for_request(&self, method: &str, path: &str) -> SubstitutionScope {
        self.owner
            .as_ref()
            .and_then(|owner| owner.scope_with(&self.target, Some((method, path))))
            .unwrap_or_else(|| self.clone())
    }

    pub(crate) fn scanner(&self) -> Scanner<'_> {
        Scanner::new(&self.request)
    }

    pub(crate) fn response_rules(&self) -> Option<ResponseRules<'_>> {
        self.response.as_ref().map(ResponseRules)
    }

    pub(crate) fn response_strip(&self) -> &ResponseHeaderStrip {
        &self.strip
    }

    pub(crate) fn target(&self) -> &TlsTarget {
        &self.target
    }

    /// Whether substituting a request body leaves its length unchanged.
    pub(crate) fn keeps_body_length(&self) -> bool {
        self.request.length_matched
    }

    pub(crate) fn injects_here(&self) -> bool {
        !self.injecting.is_empty()
    }

    pub(crate) fn plan_sigv4(
        &self,
        headers: &[(String, String)],
        target: &str,
    ) -> Result<Option<ResignPlan>, PairRefusal> {
        self.aws.plan(headers, target)
    }

    pub(crate) async fn report_sigv4_refused(&self, refused: &PairRefusal) {
        let Some(observer) = &self.observer else {
            return;
        };
        observer
            .sigv4_refused(
                self.target.host(),
                self.target.port(),
                &refused.binding,
                refused.refusal.violation_reason(),
            )
            .await;
    }

    pub(crate) fn compressed_response_policy(&self) -> CompressedResponsePolicy {
        self.compressed_response
    }

    pub(crate) async fn report(&self, bindings: &BTreeSet<SecretName>) {
        let Some(observer) = &self.observer else {
            return;
        };
        for binding in bindings {
            observer
                .sentinel_off_host(self.target.host(), self.target.port(), binding)
                .await;
        }
    }

    pub(crate) async fn report_compressed_response(&self) {
        let Some(observer) = &self.observer else {
            return;
        };
        for binding in self.injecting.iter() {
            observer
                .compressed_response(self.target.host(), self.target.port(), binding)
                .await;
        }
    }
}

/// The real to sentinel rules of one scope, from which each response gets its
/// own [`Scanner`].
#[derive(Clone, Copy)]
pub(crate) struct ResponseRules<'a>(&'a Rules);

impl<'a> ResponseRules<'a> {
    pub(crate) fn scanner(&self) -> Scanner<'a> {
        Scanner::new(self.0)
    }
}

/// Chunk-fed substitution state: output is appended to the caller's buffer,
/// and the tail that could still grow into a sentinel is carried to the next
/// [`Scanner::feed`] or flushed by [`Scanner::finish`].
pub(crate) struct Scanner<'a> {
    rules: &'a Rules,
    carry: Vec<u8>,
    flagged: BTreeSet<SecretName>,
    replaced: usize,
}

impl<'a> Scanner<'a> {
    fn new(rules: &'a Rules) -> Self {
        Self {
            rules,
            carry: Vec::new(),
            flagged: BTreeSet::new(),
            replaced: 0,
        }
    }

    pub(crate) fn feed(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        let mut work = std::mem::take(&mut self.carry);
        work.extend_from_slice(chunk);

        let mut pos = 0;
        for found in self.rules.automaton.find_iter(&work) {
            out.extend_from_slice(&work[pos..found.start()]);
            match &self.rules.actions[found.pattern().as_usize()] {
                Action::Replace(real) => {
                    out.extend_from_slice(real.expose().as_bytes());
                    self.replaced += 1;
                }
                Action::Scrub(sentinel) => {
                    out.extend_from_slice(sentinel.as_str().as_bytes());
                    self.replaced += 1;
                }
                Action::Flag(name) => {
                    self.flagged.insert(name.clone());
                    out.extend_from_slice(&work[found.range()]);
                }
            }
            pos = found.end();
        }

        let tail_start = pos.max(work.len().saturating_sub(self.rules.max_len - 1));
        out.extend_from_slice(&work[pos..tail_start]);
        self.carry = work.split_off(tail_start);
    }

    pub(crate) fn finish(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.carry);
    }

    pub(crate) fn replaced(&self) -> usize {
        self.replaced
    }

    pub(crate) fn take_flagged(&mut self) -> BTreeSet<SecretName> {
        std::mem::take(&mut self.flagged)
    }
}

#[cfg(test)]
pub(crate) mod tests;
