use std::time::Instant;

use a2a_nats::agent_id::A2aAgentId;
use trogon_audit::ReadOnly;
use trogon_identity_types::SubjectPrincipal;

use super::DecisionAudit;
use super::audit_error::AuditError;
use super::decision_event::{AllowedLayers, DecisionEvent, Verdict};
use super::deny_reason::DenyReason;

/// What one dispatch knows about the request it is deciding, so each decision
/// point states only its verdict.
///
/// With no audit attached every call is a no-op that succeeds, which is the
/// gateway's behaviour while platform audit is off.
pub struct DecisionSite<'a> {
    pub audit: Option<&'a dyn DecisionAudit>,
    pub caller: &'a SubjectPrincipal,
    pub agent_id: &'a A2aAgentId,
    /// The method in its `message/send` spelling.
    pub method: &'a str,
    pub read_only: ReadOnly,
    pub trace_id: &'a str,
    pub request_id: Option<String>,
    pub request_bytes: usize,
    pub started_mono: Instant,
}

impl DecisionSite<'_> {
    /// Records a refusal. The caller has been, or is about to be, answered with
    /// the denial, and a record that cannot be written does not change that: the
    /// writer keeps the refusal and retries it, so there is nothing to act on.
    pub async fn deny(&self, reason: DenyReason, rules_fired: &[String]) {
        let _ = self.record(Verdict::Denied(reason), rules_fired).await;
    }

    /// Records an allow before the request is forwarded. `Err` means the request
    /// must not be forwarded; it is only returned for the allows the record's
    /// class makes write-ahead, so a method declared read-only never fails here.
    pub async fn allow(&self, layers: AllowedLayers<'_>, rules_fired: &[String]) -> Result<(), AuditError> {
        self.record(Verdict::Allowed(layers), rules_fired).await
    }

    async fn record(&self, verdict: Verdict<'_>, rules_fired: &[String]) -> Result<(), AuditError> {
        let Some(audit) = self.audit else {
            return Ok(());
        };
        audit
            .record(&DecisionEvent {
                caller: self.caller,
                agent_id: self.agent_id,
                method: self.method,
                read_only: self.read_only,
                trace_id: self.trace_id,
                request_id: self.request_id.as_deref(),
                request_bytes: self.request_bytes,
                latency: self.started_mono.elapsed(),
                rules_fired,
                verdict,
            })
            .await
    }
}
