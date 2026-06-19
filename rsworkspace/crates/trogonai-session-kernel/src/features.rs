use confique::Config;

/// How runner bindings are established during model switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunnerBindingMode {
    /// Legacy `session/export` → `session/import` handoff between runners.
    #[default]
    Handoff,
    /// Hydrate the target runner from the Session Kernel projection/snapshot.
    Canonical,
}

impl RunnerBindingMode {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "canonical" | "kernel" | "session_kernel" => Self::Canonical,
            _ => Self::Handoff,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Handoff => "handoff",
            Self::Canonical => "canonical",
        }
    }
}

/// Whether the event log is authoritative for session reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventLogPrimaryMode {
    /// `state.messages` / runner KV remain operational.
    #[default]
    LegacyMessages,
    /// Event log + snapshots are primary for sessions marked event-primary.
    NewSessionsOnly,
    /// Event log is primary for all sessions that have been migrated.
    AllMigrated,
}

impl EventLogPrimaryMode {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "new_sessions" | "new_sessions_only" | "event_primary_new_sessions" => Self::NewSessionsOnly,
            "all_migrated" | "all" => Self::AllMigrated,
            _ => Self::LegacyMessages,
        }
    }
}

/// Rollout feature flags for Session Kernel integration (ADR 0007).
#[derive(Config, Clone, Debug, PartialEq, Eq)]
pub struct SessionKernelFeatureFlags {
    #[config(env = "TROGON_SESSION_KERNEL_ENABLED", default = false)]
    pub session_kernel_enabled: bool,

    /// Session Lease per `session_id` (Fase 2). Defaults on: the lease is the safety
    /// mechanism that prevents two concurrent mutations per session, so when the kernel
    /// runs it must hold leases. Disabling it reverts to the pre-Fase-2 (no-lease)
    /// behavior and is unsafe under concurrency.
    #[config(env = "TROGON_SESSION_LEASE_ENABLED", default = true)]
    pub session_lease_enabled: bool,

    /// Canonical snapshot writes to KV (Fase 3), parallel to the operational
    /// `state.messages`. Defaults on: the canonical snapshot is the materialized truth
    /// the shadow event log (Fase 4) compares against and rebuilds from, so the kernel
    /// writes it whenever it runs.
    #[config(env = "TROGON_CANONICAL_SNAPSHOT_ENABLED", default = true)]
    pub canonical_snapshot_enabled: bool,

    #[config(env = "TROGON_EVENT_LOG_SHADOW_MODE", default = true)]
    pub event_log_shadow_mode: bool,

    #[config(env = "TROGON_PROMPT_PROJECTION_ENABLED", default = true)]
    pub prompt_projection_enabled: bool,

    #[config(env = "TROGON_SWITCH_SAFETY_GATE_ENABLED", default = true)]
    pub switch_safety_gate_enabled: bool,

    #[config(env = "TROGON_CONTINUITY_CHECKPOINT_ENABLED", default = true)]
    pub continuity_checkpoint_enabled: bool,

    #[config(env = "TROGON_ARTIFACT_STORE_ENABLED", default = false)]
    pub artifact_store_enabled: bool,

    /// Per-session Event Log Compaction & Retention maintenance (§ Event Log Compaction
    /// and Retention): archive events past the retention cutoff and GC unreferenced
    /// ephemeral artifacts. Defaults off (conservative default): retention purges are an
    /// operational action enabled deliberately, never implicitly during shadow rollout.
    #[config(env = "TROGON_SESSION_MAINTENANCE_ENABLED", default = false)]
    pub session_maintenance_enabled: bool,

    #[config(env = "TROGON_RUNNER_BINDING_MODE", default = "handoff")]
    runner_binding_mode: String,

    #[config(env = "TROGON_EVENT_LOG_PRIMARY_MODE", default = "legacy_messages")]
    event_log_primary_mode: String,
}

impl SessionKernelFeatureFlags {
    pub fn runner_binding_mode(&self) -> RunnerBindingMode {
        RunnerBindingMode::parse(&self.runner_binding_mode)
    }

    pub fn event_log_primary_mode(&self) -> EventLogPrimaryMode {
        EventLogPrimaryMode::parse(&self.event_log_primary_mode)
    }

    /// Override the event-log-primary mode string, for callers/tests that build flags
    /// programmatically rather than from config/env. The raw string is parsed by
    /// [`Self::event_log_primary_mode`]; unknown values resolve to `LegacyMessages`.
    pub fn with_event_log_primary_mode(mut self, mode: impl Into<String>) -> Self {
        self.event_log_primary_mode = mode.into();
        self
    }

    /// Override the runner-binding mode string, for callers/tests that build flags
    /// programmatically rather than from config/env. Parsed by
    /// [`Self::runner_binding_mode`]; unknown values resolve to `Handoff`.
    pub fn with_runner_binding_mode(mut self, mode: impl Into<String>) -> Self {
        self.runner_binding_mode = mode.into();
        self
    }

    pub fn use_canonical_runner_binding(&self) -> bool {
        self.session_kernel_enabled && self.runner_binding_mode() == RunnerBindingMode::Canonical
    }

    /// Whether a FAILED canonical switch may silently fall back to legacy export/import
    /// handoff. Allowed only while the event log is NOT primary (legacy/shadow), as a
    /// temporary compatibility shim. Once event-primary is active, a canonical failure
    /// must surface a repairable error / rollback instead of a silent handoff
    /// (§ Rollback Strategy: "no hacer handoff silencioso"; § Compatibilidad temporal).
    pub fn allows_handoff_fallback(&self) -> bool {
        self.event_log_primary_mode() == EventLogPrimaryMode::LegacyMessages
    }

    /// Whether the Session Lease is active (Fase 2). Gated by the master kernel flag:
    /// the lease only matters when the kernel owns session mutations.
    pub fn lease_enabled(&self) -> bool {
        self.session_kernel_enabled && self.session_lease_enabled
    }

    /// Whether canonical snapshots are written to KV (Fase 3). Gated by the master
    /// kernel flag: the snapshot is only canonical truth when the kernel owns the session.
    pub fn snapshot_enabled(&self) -> bool {
        self.session_kernel_enabled && self.canonical_snapshot_enabled
    }

    pub fn safety_gate_enabled(&self) -> bool {
        self.session_kernel_enabled && self.switch_safety_gate_enabled
    }

    pub fn projection_enabled(&self) -> bool {
        self.session_kernel_enabled && self.prompt_projection_enabled
    }

    pub fn checkpoint_enabled(&self) -> bool {
        self.session_kernel_enabled && self.continuity_checkpoint_enabled
    }

    /// Whether per-session retention maintenance runs. Gated by the master kernel flag:
    /// the kernel must own the session (and its event log) before it may compact it.
    pub fn maintenance_enabled(&self) -> bool {
        self.session_kernel_enabled && self.session_maintenance_enabled
    }
}

impl Default for SessionKernelFeatureFlags {
    fn default() -> Self {
        Self::builder().load().expect("session kernel feature flag defaults")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_binding_mode_parses_canonical() {
        assert_eq!(RunnerBindingMode::parse("canonical"), RunnerBindingMode::Canonical);
        assert_eq!(RunnerBindingMode::parse("handoff"), RunnerBindingMode::Handoff);
    }

    #[test]
    fn feature_flag_defaults_are_shadow_safe() {
        let flags = SessionKernelFeatureFlags::default();
        assert!(!flags.session_kernel_enabled);
        assert!(
            flags.session_lease_enabled,
            "the lease must default on (safety mechanism)"
        );
        assert!(
            flags.canonical_snapshot_enabled,
            "the canonical snapshot must default on"
        );
        assert!(flags.event_log_shadow_mode);
        assert_eq!(flags.runner_binding_mode(), RunnerBindingMode::Handoff);
    }

    // § Fase 3 flag `canonical_snapshot_enabled`: canonical snapshots are written only
    // when the kernel owns the session (gated by the master flag), and can be toggled
    // independently.
    #[test]
    fn snapshot_enabled_is_gated_by_the_master_kernel_flag() {
        // Default: kernel off -> no canonical snapshot even though the flag is on.
        assert!(!SessionKernelFeatureFlags::default().snapshot_enabled());

        // Kernel on + snapshot on -> active.
        let on = SessionKernelFeatureFlags {
            session_kernel_enabled: true,
            ..SessionKernelFeatureFlags::default()
        };
        assert!(on.snapshot_enabled());

        // Kernel on + snapshot explicitly off -> inactive (pre-Fase-3 rollout state).
        let snapshot_off = SessionKernelFeatureFlags {
            session_kernel_enabled: true,
            canonical_snapshot_enabled: false,
            ..SessionKernelFeatureFlags::default()
        };
        assert!(!snapshot_off.snapshot_enabled());
    }

    // § Fase 2 flag `session_lease_enabled`: the lease is active only when the kernel
    // owns mutations (gated by the master flag), and can be turned off independently.
    #[test]
    fn lease_enabled_is_gated_by_the_master_kernel_flag() {
        // Default: kernel off -> lease inactive even though session_lease_enabled is on.
        assert!(!SessionKernelFeatureFlags::default().lease_enabled());

        // Kernel on + lease on -> active.
        let on = SessionKernelFeatureFlags {
            session_kernel_enabled: true,
            ..SessionKernelFeatureFlags::default()
        };
        assert!(on.lease_enabled());

        // Kernel on + lease explicitly off -> inactive (pre-Fase-2 rollout state).
        let lease_off = SessionKernelFeatureFlags {
            session_kernel_enabled: true,
            session_lease_enabled: false,
            ..SessionKernelFeatureFlags::default()
        };
        assert!(!lease_off.lease_enabled());
    }

    // § Rollback Strategy / § Compatibilidad temporal: the silent handoff fallback is a
    // feature-flag-gated rollback path — allowed only while the event log is NOT primary.
    #[test]
    fn handoff_fallback_allowed_only_while_event_log_not_primary() {
        let legacy = SessionKernelFeatureFlags::default();
        assert_eq!(legacy.event_log_primary_mode(), EventLogPrimaryMode::LegacyMessages);
        assert!(
            legacy.allows_handoff_fallback(),
            "legacy/shadow may fall back to handoff"
        );

        let new_sessions = SessionKernelFeatureFlags {
            event_log_primary_mode: "new_sessions".to_string(),
            ..SessionKernelFeatureFlags::default()
        };
        assert!(
            !new_sessions.allows_handoff_fallback(),
            "event-primary must not silently hand off"
        );

        let all_migrated = SessionKernelFeatureFlags {
            event_log_primary_mode: "all_migrated".to_string(),
            ..SessionKernelFeatureFlags::default()
        };
        assert!(!all_migrated.allows_handoff_fallback());
    }
}
