use std::time::Duration;

/// `HPe`: how long the host has to answer a side question.
pub const SIDE_QUESTION_DEADLINE: Duration = Duration::from_millis(660_000);

pub(super) const SIDE_QUESTION_TIMEOUT: &str = "side_question timed out: the host did not answer in time";

pub(super) const SIDE_QUESTION_NO_ANSWER: &str =
    "(No answer available for this side question. Try again, or ask in the main conversation.)";

pub(super) const BACKGROUND_TASKS_DISABLED: &str = "Background tasks are disabled in this session.";

/// `By`: the model verdict codes a reply may carry.
pub(super) const MODEL_CODES: [&str; 22] = [
    "invalid_request",
    "catalog_unknown",
    "not_found",
    "restricted_by_org",
    "unavailable_for_account",
    "not_offered",
    "alias_1m_disabled",
    "alias_1m_unsupported",
    "blocked_by_hook",
    "hooks_unchecked",
    "hooks_unchecked_plugin_not_loaded",
    "check_failed",
    "check_unreachable",
    "check_server_error",
    "check_rejected",
    "credits_required",
    "rate_limit",
    "auth_failed",
    "oauth_org_not_allowed",
    "model_not_current",
    "verdict_pending",
    "internal_error",
];
