use std::time::Duration;

use confique::Config;

/// NATS/JetStream operational parameters for the Session Kernel.
#[derive(Config, Clone, Debug, PartialEq)]
pub struct NatsOperationalPolicy {
    #[config(env = "TROGON_SESSION_EVENT_STREAM_MAX_BYTES", default = 1073741824)]
    pub event_stream_max_bytes: i64,

    #[config(env = "TROGON_SESSION_EVENT_STREAM_MAX_MSG_SIZE", default = 262144)]
    pub event_stream_max_message_size: i32,

    #[config(env = "TROGON_SESSION_EVENT_STREAM_REPLICAS", default = 1)]
    pub event_stream_replicas: u32,

    #[config(env = "TROGON_SESSION_KV_SNAPSHOT_HISTORY", default = 5)]
    pub kv_snapshot_history: u64,

    #[config(env = "TROGON_SESSION_OBJECT_STORE_TTL_SECS", default = 2592000)]
    object_store_ttl_secs: u64,

    #[config(env = "TROGON_SESSION_EVENT_ARCHIVE_RETENTION_DAYS", default = 90)]
    pub event_archive_retention_days: u64,

    #[config(env = "TROGON_SESSION_BACKPRESSURE_BLOCK_MUTATIONS", default = true)]
    pub backpressure_block_mutations: bool,
}

impl NatsOperationalPolicy {
    pub fn object_store_ttl(&self) -> Duration {
        Duration::from_secs(self.object_store_ttl_secs)
    }
}

/// Continuity SLO targets mapped to OpenTelemetry dashboards/alerts.
#[derive(Config, Clone, Debug, PartialEq)]
pub struct ContinuitySloPolicy {
    #[config(env = "TROGON_SLO_SWITCH_SUCCESS_RATE", default = 0.99)]
    pub switch_success_rate: f64,

    #[config(env = "TROGON_SLO_SWITCH_LATENCY_P95_MS", default = 5000)]
    pub switch_latency_p95_ms: u64,

    #[config(env = "TROGON_SLO_SWITCH_LATENCY_P95_WITH_CHECKPOINT_MS", default = 20000)]
    pub switch_latency_p95_with_checkpoint_ms: u64,

    #[config(env = "TROGON_SLO_CHECKPOINT_PASS_RATE", default = 0.95)]
    pub continuity_checkpoint_pass_rate: f64,

    #[config(env = "TROGON_SLO_REQUIRES_RECONCILIATION_RATE", default = 0.01)]
    pub requires_reconciliation_rate: f64,

    #[config(env = "TROGON_SLO_RUNNER_ATTACH_FAILURE_RATE", default = 0.01)]
    pub runner_attach_failure_rate: f64,

    #[config(env = "TROGON_SLO_ARTIFACT_MISSING_RATE", default = 0.0)]
    pub artifact_missing_rate: f64,

    /// Retries must never duplicate external side effects (doc target = 0).
    #[config(env = "TROGON_SLO_EVENT_DUPLICATE_SIDE_EFFECT_RATE", default = 0.0)]
    pub event_duplicate_side_effect_rate: f64,
}

/// Product-level operational policies for cancellation, fork/branch, and error UX.
#[derive(Config, Clone, Debug, PartialEq)]
pub struct OperationalProductPolicy {
    #[config(env = "TROGON_SESSION_CANCEL_WAIT_FOR_TOOL_RECEIPT", default = true)]
    pub cancel_wait_for_tool_receipt: bool,

    #[config(env = "TROGON_SESSION_FORCE_SWITCH_REQUIRES_ACK", default = true)]
    pub force_switch_requires_acknowledgement: bool,

    #[config(env = "TROGON_SESSION_FORK_MAX_DEPTH", default = 8)]
    pub fork_max_depth: u32,

    #[config(env = "TROGON_SESSION_SCHEMA_MIN_COMPAT_VERSION", default = 1)]
    pub schema_min_compat_version: u32,

    #[config(env = "TROGON_SESSION_EXPORT_SANITIZED_BY_DEFAULT", default = true)]
    pub export_sanitized_by_default: bool,

    #[config(env = "TROGON_SESSION_RAW_EXPORT_REQUIRES_CONFIRMATION", default = true)]
    pub raw_export_requires_confirmation: bool,
}

/// § Fallback budget y shadow divergence thresholds (cambio-modelo.md "Valores
/// iniciales cerrados"). Canonical no se promueve a default si el fallback a handoff
/// supera el presupuesto o si replay/snapshot divergen mas que el umbral. Ambos
/// umbrales deben existir antes de activar default (§ Decisiones finales cerradas).
#[derive(Config, Clone, Debug, PartialEq)]
pub struct RolloutPromotionPolicy {
    /// Max canonical/kernel-primary fallbacks to legacy handoff before promotion is
    /// blocked. Default 0: ANY fallback in a canonical session blocks until corrected
    /// (§ Fallback budget). Handoff stays allowed only in legacy/shadow/MVP.
    #[config(env = "TROGON_ROLLOUT_MAX_CANONICAL_FALLBACKS", default = 0)]
    pub max_canonical_fallbacks: usize,

    /// Max recorded minor projection divergences allowed under rollout budget. Default
    /// 0: zero tolerance. Canonical loss is NEVER under budget — only minor projection
    /// divergences may be (§ Shadow divergence).
    #[config(env = "TROGON_ROLLOUT_MAX_MINOR_PROJECTION_DIVERGENCES", default = 0)]
    pub max_minor_projection_divergences: usize,
}

impl Default for RolloutPromotionPolicy {
    fn default() -> Self {
        Self::builder().load().expect("rollout promotion policy defaults")
    }
}

/// Aggregated operational policy bundle for Session Kernel integration.
#[derive(Config, Clone, Debug, PartialEq)]
pub struct SessionKernelOperationalPolicy {
    #[config(nested)]
    pub nats: NatsOperationalPolicy,

    #[config(nested)]
    pub continuity_slos: ContinuitySloPolicy,

    #[config(nested)]
    pub product: OperationalProductPolicy,

    #[config(nested)]
    pub rollout_promotion: RolloutPromotionPolicy,
}

impl Default for SessionKernelOperationalPolicy {
    fn default() -> Self {
        Self::builder()
            .load()
            .expect("session kernel operational policy defaults")
    }
}

/// Stable product-facing error states for switch and session operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionErrorUxState {
    SessionBusy,
    SwitchBlocked,
    ConfirmationRequired,
    CapabilityMissing,
    CheckpointFailed,
    ArtifactUnavailable,
    RunnerFailed,
    SnapshotStale,
    RequiresReconciliation,
}

impl SessionErrorUxState {
    pub fn short_explanation(self) -> &'static str {
        match self {
            Self::SessionBusy => "This session is processing another operation.",
            Self::SwitchBlocked => "Model switch is blocked until pending work finishes or is reconciled.",
            Self::ConfirmationRequired => "Model switch needs your confirmation because capability may degrade.",
            Self::CapabilityMissing => "The target model lacks a capability used in this session.",
            Self::CheckpointFailed => "Continuity checkpoint did not confirm the target model understood the session.",
            Self::ArtifactUnavailable => "A referenced artifact is missing or not yet persisted.",
            Self::RunnerFailed => "The runner binding failed; session state is preserved in Trogonai.",
            Self::SnapshotStale => "Session snapshot is stale; Trogonai will rebuild from the event log.",
            Self::RequiresReconciliation => "An operation needs manual reconciliation before continuing safely.",
        }
    }

    pub fn recommended_action(self) -> &'static str {
        match self {
            Self::SessionBusy => "Wait, cancel the in-flight operation, or retry when the session is idle.",
            Self::SwitchBlocked => "Finish or cancel pending tools, save work, then retry the switch.",
            Self::ConfirmationRequired => "Review warnings and confirm to proceed or pick another model.",
            Self::CapabilityMissing => "Choose a model with the required capability or accept explicit degradation.",
            Self::CheckpointFailed => "Retry with more context, repair artifacts, or continue with caution.",
            Self::ArtifactUnavailable => "Re-run the tool or restore the artifact before risky actions.",
            Self::RunnerFailed => "Retry the switch or continue on the previous runner binding.",
            Self::SnapshotStale => "Continue; Trogonai will regenerate the snapshot in the background.",
            Self::RequiresReconciliation => "Inspect pending tool state and confirm how to proceed.",
        }
    }

    /// The real impact on the user's work (Error UX Policy attribute).
    pub fn real_impact(self) -> &'static str {
        match self {
            Self::SessionBusy => "The switch cannot start until the current operation finishes.",
            Self::SwitchBlocked => "The switch will not proceed and the session stays on the current model.",
            Self::ConfirmationRequired => "Some context or capability will be degraded after the switch.",
            Self::CapabilityMissing => "That capability will be unavailable or degraded on the target model.",
            Self::CheckpointFailed => "The target model may act without fully understanding the session.",
            Self::ArtifactUnavailable => "That content is not available in the projection right now.",
            Self::RunnerFailed => "The switch did not complete; the canonical session is preserved.",
            Self::SnapshotStale => "A view may be briefly out of date while the snapshot rebuilds.",
            Self::RequiresReconciliation => "A non-idempotent operation may have partially applied.",
        }
    }

    /// The options available to the user (Error UX Policy attribute).
    pub fn available_options(self) -> &'static [&'static str] {
        match self {
            Self::SessionBusy => &["wait", "cancel", "retry_when_idle"],
            Self::SwitchBlocked => &["wait", "cancel", "persist_pending"],
            Self::ConfirmationRequired => &["confirm", "choose_other_model", "cancel"],
            Self::CapabilityMissing => &["confirm_with_degradation", "choose_other_model", "cancel"],
            Self::CheckpointFailed => &["retry_with_more_context", "keep_current_model", "cancel"],
            Self::ArtifactUnavailable => &["continue_without", "regenerate"],
            Self::RunnerFailed => &["retry", "keep_current_model"],
            Self::SnapshotStale => &["wait", "retry"],
            Self::RequiresReconciliation => &["inspect", "confirm_continue", "cancel"],
        }
    }

    /// Whether it is safe to keep working in this state (Error UX Policy attribute).
    pub fn safe_to_continue(self) -> bool {
        match self {
            // Blocked / busy / unsafe states require resolution first.
            Self::SessionBusy | Self::SwitchBlocked | Self::CheckpointFailed | Self::RequiresReconciliation => false,
            // Degradations and recoverable states: safe to continue, with awareness.
            Self::ConfirmationRequired
            | Self::CapabilityMissing
            | Self::ArtifactUnavailable
            | Self::RunnerFailed
            | Self::SnapshotStale => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_policy_defaults_load() {
        let policy = SessionKernelOperationalPolicy::default();
        assert_eq!(policy.nats.event_stream_replicas, 1);
        assert!(policy.continuity_slos.switch_success_rate > 0.98);
        // Retries must never duplicate external side effects (doc target = 0).
        assert_eq!(policy.continuity_slos.event_duplicate_side_effect_rate, 0.0);
        assert_eq!(policy.continuity_slos.artifact_missing_rate, 0.0);
        assert!(policy.product.export_sanitized_by_default);
    }

    #[test]
    fn error_ux_states_have_actionable_guidance() {
        assert!(
            SessionErrorUxState::SwitchBlocked
                .recommended_action()
                .contains("pending")
        );
    }

    #[test]
    fn every_error_ux_state_has_all_five_attributes() {
        // Error UX Policy: every state must include a short explanation, the real
        // impact, a recommended action, available options, and whether it is safe
        // to continue.
        let states = [
            SessionErrorUxState::SessionBusy,
            SessionErrorUxState::SwitchBlocked,
            SessionErrorUxState::ConfirmationRequired,
            SessionErrorUxState::CapabilityMissing,
            SessionErrorUxState::CheckpointFailed,
            SessionErrorUxState::ArtifactUnavailable,
            SessionErrorUxState::RunnerFailed,
            SessionErrorUxState::SnapshotStale,
            SessionErrorUxState::RequiresReconciliation,
        ];
        for state in states {
            assert!(!state.short_explanation().is_empty());
            assert!(!state.real_impact().is_empty());
            assert!(!state.recommended_action().is_empty());
            assert!(!state.available_options().is_empty());
            // safe_to_continue is a bool; just exercise it.
            let _ = state.safe_to_continue();
        }
        // Blocked/unsafe states are not safe to continue; degradations are.
        assert!(!SessionErrorUxState::SwitchBlocked.safe_to_continue());
        assert!(SessionErrorUxState::ConfirmationRequired.safe_to_continue());
    }
}
