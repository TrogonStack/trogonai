//! Phase 3 — DLQ **`caller_id`** from JWT; optional gateway-side DLQ mirroring.
//!
//! Minted NATS User JWTs carry a stable **`caller_id`** claim used as the middle token in
//! **`{prefix}.push.dlq.{caller_id}.{task_id}`** (see
//! [`A2A_PUSH_DLQ_OPS.md`](../../../../docs/A2A_PUSH_DLQ_OPS.md)). Terminal push DLQ publishes
//! today originate on the agent **`Bridge`** in **`a2a-nats`**, not on gateway ingress. When
//! gateway-owned push remediation lands, an opt-in mirror may publish the same JSON envelope
//! onto the same subject shape for operator visibility.
//!
//! **Related:** [`p0_auth_callout_service`](super::p0_auth_callout_service) ·
//! [`A2A_AUTH_CALLOUT_SKETCH.md`](../../../../docs/A2A_AUTH_CALLOUT_SKETCH.md) ·
//! [`A2A_TODO.md`](../../../../A2A_TODO.md)

/// Fallback DLQ `{caller_id}` segment when no JWT-derived identity is available.
///
/// Matches [`a2a_nats::constants::DEFAULT_PUSH_DLQ_CALLER_SEGMENT`] and
/// [`A2A_PUSH_DLQ_OPS.md`](../../../../docs/A2A_PUSH_DLQ_OPS.md) embedder guidance.
pub const DEFAULT_CALLER_SEGMENT_UNAUTH: &str = "_";

/// Extracts the DLQ routing **`caller_id`** segment from minted NATS User JWT context.
pub trait CallerIdExtractor {
    /// Returns the JWT-derived caller segment when authenticated; `None` when unauthenticated.
    fn caller_segment(&self) -> Option<&str>;
}

/// Resolved `{prefix}.push.dlq.{caller_id}.{task_id}` subject fragments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlqSubjectParts {
    pub prefix: &'static str,
    pub caller_id: String,
    pub task_id: String,
}

impl DlqSubjectParts {
    /// Builds DLQ subject parts using JWT-derived caller identity when present.
    pub fn from_caller<E: CallerIdExtractor>(
        prefix: &'static str,
        extractor: &E,
        task_id: impl Into<String>,
    ) -> Self {
        Self {
            prefix,
            caller_id: resolve_caller_segment(extractor.caller_segment()).to_owned(),
            task_id: task_id.into(),
        }
    }

    /// Builds DLQ subject parts from an explicit caller segment (env override or propagated config).
    pub fn new(prefix: &'static str, caller_id: impl Into<String>, task_id: impl Into<String>) -> Self {
        let caller_id = caller_id.into();
        Self {
            prefix,
            caller_id: resolve_caller_segment(Some(caller_id.as_str())).to_owned(),
            task_id: task_id.into(),
        }
    }

    /// Publish subject: **`{prefix}.push.dlq.{caller_id}.{task_id}`**.
    pub fn publish_subject(&self) -> String {
        format!("{}.push.dlq.{}.{}", self.prefix, self.caller_id, self.task_id)
    }
}

/// Resolves the effective caller segment, falling back to [`DEFAULT_CALLER_SEGMENT_UNAUTH`].
pub fn resolve_caller_segment(extracted: Option<&str>) -> &str {
    extracted
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .unwrap_or(DEFAULT_CALLER_SEGMENT_UNAUTH)
}

/// Policy gate for optional gateway-side DLQ mirror publishes (future push remediation path).
pub trait GatewayDlqMirroringPolicy {
    /// When `true`, gateway may publish a mirror DLQ record after observing terminal push failure.
    fn mirror_terminal_push_failure(&self) -> bool;
}

/// Compile-time intent for a future gateway DLQ mirror publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayDlqMirrorIntent {
    pub parts: DlqSubjectParts,
    pub should_publish: bool,
}

/// Derives DLQ routing parts and whether the gateway should mirror a terminal push failure.
pub fn gateway_dlq_mirror_intent<P, E>(
    policy: &P,
    prefix: &'static str,
    extractor: &E,
    task_id: impl Into<String>,
) -> GatewayDlqMirrorIntent
where
    P: GatewayDlqMirroringPolicy,
    E: CallerIdExtractor,
{
    GatewayDlqMirrorIntent {
        parts: DlqSubjectParts::from_caller(prefix, extractor, task_id),
        should_publish: policy.mirror_terminal_push_failure(),
    }
}

/// Default policy — gateway does not mirror DLQ traffic (current production behavior).
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverMirrorGatewayDlq;

impl GatewayDlqMirroringPolicy for NeverMirrorGatewayDlq {
    fn mirror_terminal_push_failure(&self) -> bool {
        false
    }
}

/// Opt-in policy for future gateway-side DLQ mirror publishes.
#[derive(Debug, Clone, Copy, Default)]
pub struct MirrorTerminalPushFailureGatewayDlq;

impl GatewayDlqMirroringPolicy for MirrorTerminalPushFailureGatewayDlq {
    fn mirror_terminal_push_failure(&self) -> bool {
        true
    }
}

/// Stub JWT caller extractor — always unauthenticated until auth-callout wiring lands.
#[derive(Debug, Clone, Copy, Default)]
pub struct StubCallerIdExtractor;

impl CallerIdExtractor for StubCallerIdExtractor {
    fn caller_segment(&self) -> Option<&str> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedCallerIdExtractor(&'static str);

    impl CallerIdExtractor for FixedCallerIdExtractor {
        fn caller_segment(&self) -> Option<&str> {
            Some(self.0)
        }
    }

    #[test]
    fn resolve_caller_segment_falls_back_to_default() {
        assert_eq!(resolve_caller_segment(None), DEFAULT_CALLER_SEGMENT_UNAUTH);
        assert_eq!(resolve_caller_segment(Some("")), DEFAULT_CALLER_SEGMENT_UNAUTH);
        assert_eq!(resolve_caller_segment(Some("   ")), DEFAULT_CALLER_SEGMENT_UNAUTH);
    }

    #[test]
    fn resolve_caller_segment_trims_jwt_claim() {
        assert_eq!(resolve_caller_segment(Some(" oidc-sub-7 ")), "oidc-sub-7");
    }

    #[test]
    fn dlq_subject_parts_from_jwt_caller_id() {
        let parts = DlqSubjectParts::from_caller("a2a", &FixedCallerIdExtractor("alice"), "task-1");
        assert_eq!(parts.publish_subject(), "a2a.push.dlq.alice.task-1");
    }

    #[test]
    fn dlq_subject_parts_unauth_uses_default_segment() {
        let parts = DlqSubjectParts::from_caller("a2a", &StubCallerIdExtractor, "task-2");
        assert_eq!(parts.publish_subject(), "a2a.push.dlq._.task-2");
    }

    #[test]
    fn gateway_mirror_intent_respects_policy() {
        let intent = gateway_dlq_mirror_intent(
            &NeverMirrorGatewayDlq,
            "a2a",
            &FixedCallerIdExtractor("bob"),
            "task-3",
        );
        assert!(!intent.should_publish);
        assert_eq!(intent.parts.publish_subject(), "a2a.push.dlq.bob.task-3");

        let mirror = gateway_dlq_mirror_intent(
            &MirrorTerminalPushFailureGatewayDlq,
            "a2a",
            &FixedCallerIdExtractor("bob"),
            "task-3",
        );
        assert!(mirror.should_publish);
    }
}
