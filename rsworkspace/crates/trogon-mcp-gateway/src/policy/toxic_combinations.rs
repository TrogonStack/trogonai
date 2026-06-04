//! Session-scoped deny list for tool combinations that are dangerous
//! only when paired (e.g. `read_secrets` + `send_email` = exfiltration).
//!
//! Per `GCP_TODO.md §3.5`:
//! - rule shape `{ tools, reason }`;
//! - evaluated at session level — the first call that **completes**
//!   a forbidden set is denied;
//! - default rule-set is empty (operators opt in).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToxicCombinationRule {
    pub tools: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToxicCombinationsConfig {
    #[serde(default, rename = "deny_combinations")]
    pub rules: Vec<ToxicCombinationRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToxicCombinationDeny {
    pub reason: String,
    pub matched_tools: Vec<String>,
}

/// Tracks the tool-call set per session and denies the first call
/// that completes a forbidden combination.
#[derive(Debug, Default)]
pub struct ToxicCombinationsPolicy {
    rules: Vec<NormalizedRule>,
    sessions: Mutex<HashMap<String, HashSet<String>>>,
}

#[derive(Debug, Clone)]
struct NormalizedRule {
    tools: HashSet<String>,
    reason: String,
}

impl ToxicCombinationsPolicy {
    pub fn new(config: ToxicCombinationsConfig) -> Self {
        let rules = config
            .rules
            .into_iter()
            .filter(|r| !r.tools.is_empty())
            .map(|r| NormalizedRule {
                tools: r.tools.into_iter().collect(),
                reason: r.reason,
            })
            .collect();
        Self {
            rules,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// `true` if no rules are configured — caller can skip the lock.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Record a tool call on `session_id` and return `Some(deny)`
    /// when that call completes a forbidden combination. The tool
    /// is recorded either way so a follow-up retry hits the same
    /// deny decision.
    pub fn record_and_check(&self, session_id: &str, tool: &str) -> Option<ToxicCombinationDeny> {
        if self.rules.is_empty() {
            return None;
        }
        let mut sessions = self.sessions.lock().expect("toxic-combo session lock");
        let observed = sessions.entry(session_id.to_string()).or_default();
        observed.insert(tool.to_string());
        for rule in &self.rules {
            if rule.tools.is_subset(observed) {
                let mut matched: Vec<String> = rule.tools.iter().cloned().collect();
                matched.sort();
                return Some(ToxicCombinationDeny {
                    reason: rule.reason.clone(),
                    matched_tools: matched,
                });
            }
        }
        None
    }

    pub fn forget_session(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().expect("toxic-combo session lock");
        sessions.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exfil_policy() -> ToxicCombinationsPolicy {
        ToxicCombinationsPolicy::new(ToxicCombinationsConfig {
            rules: vec![ToxicCombinationRule {
                tools: vec!["send_email".into(), "read_secrets".into()],
                reason: "exfiltration risk".into(),
            }],
        })
    }

    #[test]
    fn first_call_in_combination_is_allowed() {
        let policy = exfil_policy();
        assert!(policy.record_and_check("session-a", "read_secrets").is_none());
    }

    #[test]
    fn completing_call_is_denied_with_reason() {
        let policy = exfil_policy();
        assert!(policy.record_and_check("session-a", "read_secrets").is_none());
        let deny = policy
            .record_and_check("session-a", "send_email")
            .expect("must deny");
        assert_eq!(deny.reason, "exfiltration risk");
        assert_eq!(
            deny.matched_tools,
            vec!["read_secrets".to_string(), "send_email".to_string()]
        );
    }

    #[test]
    fn sessions_are_isolated() {
        let policy = exfil_policy();
        assert!(policy.record_and_check("session-a", "read_secrets").is_none());
        assert!(policy.record_and_check("session-b", "send_email").is_none());
    }

    #[test]
    fn unrelated_tools_do_not_trip_the_rule() {
        let policy = exfil_policy();
        assert!(policy.record_and_check("session-a", "list_files").is_none());
        assert!(policy.record_and_check("session-a", "search_index").is_none());
    }

    #[test]
    fn forget_session_clears_observed_tools() {
        let policy = exfil_policy();
        assert!(policy.record_and_check("session-a", "read_secrets").is_none());
        policy.forget_session("session-a");
        assert!(policy.record_and_check("session-a", "send_email").is_none());
    }

    #[test]
    fn empty_rule_set_is_a_noop() {
        let policy = ToxicCombinationsPolicy::new(ToxicCombinationsConfig::default());
        assert!(policy.is_empty());
        assert!(policy.record_and_check("s", "anything").is_none());
    }

    #[test]
    fn single_tool_rule_denies_on_first_call() {
        let policy = ToxicCombinationsPolicy::new(ToxicCombinationsConfig {
            rules: vec![ToxicCombinationRule {
                tools: vec!["delete_database".into()],
                reason: "irreversible".into(),
            }],
        });
        let deny = policy
            .record_and_check("s", "delete_database")
            .expect("single-tool rule denies");
        assert_eq!(deny.reason, "irreversible");
    }

    #[test]
    fn three_tool_combination_requires_all_three() {
        let policy = ToxicCombinationsPolicy::new(ToxicCombinationsConfig {
            rules: vec![ToxicCombinationRule {
                tools: vec!["a".into(), "b".into(), "c".into()],
                reason: "triple".into(),
            }],
        });
        assert!(policy.record_and_check("s", "a").is_none());
        assert!(policy.record_and_check("s", "b").is_none());
        let deny = policy.record_and_check("s", "c").expect("must deny on c");
        assert_eq!(deny.reason, "triple");
    }

    #[test]
    fn config_deserializes_deny_combinations_field() {
        let json = serde_json::json!({
            "deny_combinations": [
                { "tools": ["send_email", "read_secrets"], "reason": "exfiltration risk" }
            ]
        });
        let parsed: ToxicCombinationsConfig = serde_json::from_value(json).expect("parse");
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(parsed.rules[0].reason, "exfiltration risk");
    }
}
