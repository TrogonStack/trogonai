//! Feature flag definitions for `trogon-agent`.
//!
//! Flags follow the Spacedrive-inspired pattern from `trogon-splitio`: a typed
//! enum where each variant maps to a string name configured in the Split.io /
//! Harness FME dashboard.  This gives compile-time safety — flag names can
//! never be misspelled at call sites.
//!
//! ## Fail-open contract
//!
//! All flags default to **`true` (enabled)** when Split.io is not configured
//! (`SPLIT_EVALUATOR_URL` unset).  This ensures the agent works out-of-the-box
//! without requiring a Split.io account; the evaluator is purely additive.
//!
//! ## Flags
//!
//! | Flag name                        | Controls                              |
//! |----------------------------------|---------------------------------------|
//! | `agent_pr_review_enabled`        | PR review & PR merged fallback handlers |
//! | `agent_comment_handler_enabled`  | Issue / PR comment fallback handler   |
//! | `agent_push_handler_enabled`     | Push-to-branch fallback handler       |
//! | `agent_ci_handler_enabled`       | CI-completed fallback handler         |
//! | `agent_issue_triage_enabled`     | Linear issue-triage fallback handler  |
//! | `agent_alert_handler_enabled`    | Datadog alert fallback handler        |
//! | `agent_memory_enabled`           | Fetching `.trogon/memory.md` for all handlers |
//! | `agent_mcp_enabled`              | Loading MCP server tools at startup   |

use trogon_splitio::flags::FeatureFlag;

/// All feature flags that control `trogon-agent` behaviour.
///
/// Implement each variant in the Split.io / Harness FME dashboard using the
/// string returned by [`FeatureFlag::name`].  Set a flag to `"off"` to
/// disable the corresponding handler or capability without redeploying.
pub enum AgentFlag {
    /// Enable/disable the PR review and PR merged fallback handlers.
    PrReviewEnabled,
    /// Enable/disable the GitHub issue/PR comment fallback handler.
    CommentHandlerEnabled,
    /// Enable/disable the GitHub push-to-branch fallback handler.
    PushHandlerEnabled,
    /// Enable/disable the GitHub CI-completed fallback handler.
    CiHandlerEnabled,
    /// Enable/disable the Linear issue-triage fallback handler.
    IssueTriageEnabled,
    /// Enable/disable the Datadog alert fallback handler.
    AlertHandlerEnabled,
    /// Enable/disable fetching `.trogon/memory.md` before each handler run.
    MemoryEnabled,
    /// Enable/disable MCP server tools (loaded at startup).
    McpEnabled,
}

impl FeatureFlag for AgentFlag {
    fn name(&self) -> &'static str {
        match self {
            Self::PrReviewEnabled       => "agent_pr_review_enabled",
            Self::CommentHandlerEnabled => "agent_comment_handler_enabled",
            Self::PushHandlerEnabled    => "agent_push_handler_enabled",
            Self::CiHandlerEnabled      => "agent_ci_handler_enabled",
            Self::IssueTriageEnabled    => "agent_issue_triage_enabled",
            Self::AlertHandlerEnabled   => "agent_alert_handler_enabled",
            Self::MemoryEnabled         => "agent_memory_enabled",
            Self::McpEnabled            => "agent_mcp_enabled",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_names_are_correct() {
        assert_eq!(AgentFlag::PrReviewEnabled.name(),       "agent_pr_review_enabled");
        assert_eq!(AgentFlag::CommentHandlerEnabled.name(), "agent_comment_handler_enabled");
        assert_eq!(AgentFlag::PushHandlerEnabled.name(),    "agent_push_handler_enabled");
        assert_eq!(AgentFlag::CiHandlerEnabled.name(),      "agent_ci_handler_enabled");
        assert_eq!(AgentFlag::IssueTriageEnabled.name(),    "agent_issue_triage_enabled");
        assert_eq!(AgentFlag::AlertHandlerEnabled.name(),   "agent_alert_handler_enabled");
        assert_eq!(AgentFlag::MemoryEnabled.name(),         "agent_memory_enabled");
        assert_eq!(AgentFlag::McpEnabled.name(),            "agent_mcp_enabled");
    }

    #[test]
    fn description_defaults_to_name() {
        assert_eq!(AgentFlag::PrReviewEnabled.description(), AgentFlag::PrReviewEnabled.name());
        assert_eq!(AgentFlag::MemoryEnabled.description(), AgentFlag::MemoryEnabled.name());
    }
}
