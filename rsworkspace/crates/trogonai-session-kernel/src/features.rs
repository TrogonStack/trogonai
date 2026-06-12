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
            "new_sessions" | "new_sessions_only" | "event_primary_new_sessions" => {
                Self::NewSessionsOnly
            }
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

    #[config(env = "TROGON_EVENT_LOG_SHADOW_MODE", default = true)]
    pub event_log_shadow_mode: bool,

    #[config(env = "TROGON_PROMPT_PROJECTION_ENABLED", default = false)]
    pub prompt_projection_enabled: bool,

    #[config(env = "TROGON_SWITCH_SAFETY_GATE_ENABLED", default = true)]
    pub switch_safety_gate_enabled: bool,

    #[config(env = "TROGON_CONTINUITY_CHECKPOINT_ENABLED", default = false)]
    pub continuity_checkpoint_enabled: bool,

    #[config(env = "TROGON_ARTIFACT_STORE_ENABLED", default = false)]
    pub artifact_store_enabled: bool,

    #[config(env = "TROGON_RUNNER_BINDING_MODE", default = "handoff")]
    runner_binding_mode: String,

    #[config(
        env = "TROGON_EVENT_LOG_PRIMARY_MODE",
        default = "legacy_messages"
    )]
    event_log_primary_mode: String,
}

impl SessionKernelFeatureFlags {
    pub fn runner_binding_mode(&self) -> RunnerBindingMode {
        RunnerBindingMode::parse(&self.runner_binding_mode)
    }

    pub fn event_log_primary_mode(&self) -> EventLogPrimaryMode {
        EventLogPrimaryMode::parse(&self.event_log_primary_mode)
    }

    pub fn use_canonical_runner_binding(&self) -> bool {
        self.session_kernel_enabled && self.runner_binding_mode() == RunnerBindingMode::Canonical
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
}

impl Default for SessionKernelFeatureFlags {
    fn default() -> Self {
        Self::builder()
            .load()
            .expect("session kernel feature flag defaults")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_binding_mode_parses_canonical() {
        assert_eq!(
            RunnerBindingMode::parse("canonical"),
            RunnerBindingMode::Canonical
        );
        assert_eq!(
            RunnerBindingMode::parse("handoff"),
            RunnerBindingMode::Handoff
        );
    }

    #[test]
    fn feature_flag_defaults_are_shadow_safe() {
        let flags = SessionKernelFeatureFlags::default();
        assert!(!flags.session_kernel_enabled);
        assert!(flags.event_log_shadow_mode);
        assert_eq!(flags.runner_binding_mode(), RunnerBindingMode::Handoff);
    }
}
