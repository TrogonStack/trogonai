use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EgressAction {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EgressRule {
    pub host_pattern: String,
    pub action: EgressAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EgressPolicy {
    pub default_action: EgressAction,
    pub rules: Vec<EgressRule>,
}

impl EgressPolicy {
    pub fn is_allowed(&self, url: &str) -> bool {
        let host = match reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        {
            Some(h) => h,
            None => return false,
        };

        if is_link_local(&host) {
            return false;
        }

        for rule in &self.rules {
            if pattern_matches(&rule.host_pattern, &host) {
                return rule.action == EgressAction::Allow;
            }
        }

        self.default_action == EgressAction::Allow
    }

    pub fn default_safe() -> Self {
        Self {
            default_action: EgressAction::Allow,
            rules: vec![
                EgressRule {
                    host_pattern: "169.254.*".to_string(),
                    action: EgressAction::Deny,
                },
                EgressRule {
                    host_pattern: "169.254.169.254".to_string(),
                    action: EgressAction::Deny,
                },
            ],
        }
    }
}

fn is_link_local(host: &str) -> bool {
    if host == "169.254.169.254" {
        return true;
    }
    if let Some(rest) = host.strip_prefix("169.254.") {
        if !rest.is_empty() {
            return true;
        }
    }
    false
}

fn pattern_matches(pattern: &str, host: &str) -> bool {
    let pattern_lower = pattern.to_lowercase();
    if let Some(suffix) = pattern_lower.strip_prefix("*.") {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        host == pattern_lower.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_rule_permits_matching_host() {
        let policy = EgressPolicy {
            default_action: EgressAction::Deny,
            rules: vec![EgressRule {
                host_pattern: "api.anthropic.com".to_string(),
                action: EgressAction::Allow,
            }],
        };
        assert!(policy.is_allowed("https://api.anthropic.com/v1/messages"));
    }

    #[test]
    fn deny_rule_blocks_matching_host() {
        let policy = EgressPolicy {
            default_action: EgressAction::Allow,
            rules: vec![EgressRule {
                host_pattern: "evil.internal".to_string(),
                action: EgressAction::Deny,
            }],
        };
        assert!(!policy.is_allowed("http://evil.internal/steal"));
    }

    #[test]
    fn default_action_allow_permits_unmatched_host() {
        let policy = EgressPolicy {
            default_action: EgressAction::Allow,
            rules: vec![],
        };
        assert!(policy.is_allowed("https://example.com/api"));
    }

    #[test]
    fn default_action_deny_blocks_unmatched_host() {
        let policy = EgressPolicy {
            default_action: EgressAction::Deny,
            rules: vec![],
        };
        assert!(!policy.is_allowed("https://example.com/api"));
    }

    #[test]
    fn wildcard_rule_matches_subdomain() {
        let policy = EgressPolicy {
            default_action: EgressAction::Deny,
            rules: vec![EgressRule {
                host_pattern: "*.internal".to_string(),
                action: EgressAction::Allow,
            }],
        };
        assert!(policy.is_allowed("https://service.internal/health"));
        assert!(!policy.is_allowed("https://external.com/api"));
    }

    #[test]
    fn wildcard_rule_does_not_match_exact_suffix_without_dot() {
        let policy = EgressPolicy {
            default_action: EgressAction::Allow,
            rules: vec![EgressRule {
                host_pattern: "*.internal".to_string(),
                action: EgressAction::Deny,
            }],
        };
        assert!(policy.is_allowed("https://notinternal.com/api"));
    }

    #[test]
    fn first_match_wins_allow_before_deny() {
        let policy = EgressPolicy {
            default_action: EgressAction::Deny,
            rules: vec![
                EgressRule {
                    host_pattern: "allowed.internal".to_string(),
                    action: EgressAction::Allow,
                },
                EgressRule {
                    host_pattern: "*.internal".to_string(),
                    action: EgressAction::Deny,
                },
            ],
        };
        assert!(policy.is_allowed("https://allowed.internal/api"));
        assert!(!policy.is_allowed("https://other.internal/api"));
    }

    #[test]
    fn hard_block_169_254_169_254_regardless_of_rules() {
        let policy = EgressPolicy {
            default_action: EgressAction::Allow,
            rules: vec![EgressRule {
                host_pattern: "169.254.169.254".to_string(),
                action: EgressAction::Allow,
            }],
        };
        assert!(!policy.is_allowed("http://169.254.169.254/latest/meta-data/"));
    }

    #[test]
    fn hard_block_link_local_subnet() {
        let policy = EgressPolicy {
            default_action: EgressAction::Allow,
            rules: vec![EgressRule {
                host_pattern: "169.254.*".to_string(),
                action: EgressAction::Allow,
            }],
        };
        assert!(!policy.is_allowed("http://169.254.1.1/"));
        assert!(!policy.is_allowed("http://169.254.0.1/secret"));
    }

    #[test]
    fn invalid_url_is_denied() {
        let policy = EgressPolicy {
            default_action: EgressAction::Allow,
            rules: vec![],
        };
        assert!(!policy.is_allowed("not-a-url"));
        assert!(!policy.is_allowed(""));
    }

    #[test]
    fn default_safe_denies_metadata_endpoint() {
        let policy = EgressPolicy::default_safe();
        assert!(!policy.is_allowed("http://169.254.169.254/latest/meta-data/"));
    }

    #[test]
    fn default_safe_denies_link_local_subnet() {
        let policy = EgressPolicy::default_safe();
        assert!(!policy.is_allowed("http://169.254.100.200/anything"));
    }

    #[test]
    fn default_safe_allows_public_host() {
        let policy = EgressPolicy::default_safe();
        assert!(policy.is_allowed("https://api.example.com/v1"));
    }

    #[test]
    fn egress_policy_serde_roundtrip() {
        let policy = EgressPolicy {
            default_action: EgressAction::Deny,
            rules: vec![EgressRule {
                host_pattern: "*.anthropic.com".to_string(),
                action: EgressAction::Allow,
            }],
        };
        let json = serde_json::to_string(&policy).unwrap();
        let back: EgressPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, back);
    }

    #[test]
    fn egress_action_serde_snake_case() {
        let allow = serde_json::to_string(&EgressAction::Allow).unwrap();
        let deny = serde_json::to_string(&EgressAction::Deny).unwrap();
        assert_eq!(allow, "\"allow\"");
        assert_eq!(deny, "\"deny\"");
    }
}
