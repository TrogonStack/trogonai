//! Codex permission-mode boundary helpers.
//!
//! # Architectural limitation
//!
//! Trogon ACP permission modes (`default`, `acceptEdits`, `plan`, …) are accepted at the
//! runner edge and mapped to the coarse knobs the Codex `app-server` exposes (primarily
//! `approvalPolicy` on `turn/start`). The subprocess runs tools **in-process**; Trogon only
//! observes `item/started` / `item/completed` notifications and cannot pre-gate individual
//! tool calls the way [`trogon-runner-tools`] does for acp/xai/openrouter runners.
//!
//! Session-level mapping plus refusing modes Trogon cannot honor even coarsely is the only
//! enforcement Trogon controls here. Finer per-tool gating inside the subprocess is a known
//! architectural limitation, not a bug.

/// Codex `approvalPolicy` values (kebab-case on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexApprovalPolicy {
    OnRequest,
    UnlessTrusted,
    Never,
}

impl CodexApprovalPolicy {
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::OnRequest => "on-request",
            Self::UnlessTrusted => "unless-trusted",
            Self::Never => "never",
        }
    }
}

/// Action Trogon takes when a session mode hits a boundary it controls (`set_session_mode`,
/// `prompt` / `turn/start`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeGateAction {
    /// Map the Trogon mode to a Codex `approvalPolicy` for the next turn.
    Forward {
        approval_policy: CodexApprovalPolicy,
        /// Log a one-line warning that per-tool gating is not enforced in-subprocess.
        warn_unenforced: bool,
    },
    /// Mode cannot be honored even at the coarse subprocess boundary — refuse the operation.
    Refuse { reason: &'static str },
}

/// Map a validated Trogon session mode to the coarse gate action at session/turn start.
///
/// Fully restrictive modes such as `plan` (read-only in other runners) cannot be enforced
/// because Codex executes tools internally; Trogon refuses rather than silently pretending.
pub fn mode_gate_action(mode: &str) -> ModeGateAction {
    match mode {
        "plan" => ModeGateAction::Refuse {
            reason: "plan mode is not supported for the codex runner: tool execution runs \
                     inside the codex app-server subprocess and Trogon cannot pre-gate \
                     individual tool calls; use acp, xai, or openrouter for read-only plan mode",
        },
        "default" => ModeGateAction::Forward {
            approval_policy: CodexApprovalPolicy::OnRequest,
            warn_unenforced: true,
        },
        "acceptEdits" | "dontAsk" => ModeGateAction::Forward {
            approval_policy: CodexApprovalPolicy::UnlessTrusted,
            warn_unenforced: true,
        },
        "bypassPermissions" => ModeGateAction::Forward {
            approval_policy: CodexApprovalPolicy::Never,
            warn_unenforced: true,
        },
        _ => ModeGateAction::Refuse {
            reason: "unknown permission mode",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_mode_is_refused() {
        assert!(matches!(
            mode_gate_action("plan"),
            ModeGateAction::Refuse { .. }
        ));
    }

    #[test]
    fn default_maps_to_on_request_with_warning() {
        assert_eq!(
            mode_gate_action("default"),
            ModeGateAction::Forward {
                approval_policy: CodexApprovalPolicy::OnRequest,
                warn_unenforced: true,
            }
        );
    }

    #[test]
    fn accept_edits_maps_to_unless_trusted() {
        assert_eq!(
            mode_gate_action("acceptEdits"),
            ModeGateAction::Forward {
                approval_policy: CodexApprovalPolicy::UnlessTrusted,
                warn_unenforced: true,
            }
        );
    }

    #[test]
    fn bypass_maps_to_never() {
        assert_eq!(
            mode_gate_action("bypassPermissions"),
            ModeGateAction::Forward {
                approval_policy: CodexApprovalPolicy::Never,
                warn_unenforced: true,
            }
        );
    }

    #[test]
    fn unknown_mode_is_refused() {
        assert!(matches!(
            mode_gate_action("full-auto"),
            ModeGateAction::Refuse { .. }
        ));
    }
}
