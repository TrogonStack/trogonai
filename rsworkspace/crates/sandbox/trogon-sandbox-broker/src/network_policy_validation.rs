use trogon_sandbox_contract::{DomainRule, DomainRuleAction, MatchRule, NetworkPolicy, StringMatcher};

mod network_policy_validation_error;

pub use network_policy_validation_error::NetworkPolicyValidationError;

/// Validates a [`NetworkPolicy`] at lease-request time. `AllowAll`/`DenyAll` are
/// always valid; `UserDefined` has its match-rule regexes compiled so a
/// malformed one is rejected before the lease is dispatched to a worker. CIDR
/// ranges, forward URLs and domain patterns are parsed by their value objects
/// when the policy is deserialised, so they never reach this function. This
/// performs parse-validity checks only, no enforcement.
pub fn validate_network_policy(policy: &NetworkPolicy) -> Result<(), NetworkPolicyValidationError> {
    let NetworkPolicy::UserDefined { rules, .. } = policy else {
        return Ok(());
    };

    for rule in rules {
        validate_domain_rule(rule)?;
    }

    Ok(())
}

fn validate_domain_rule(rule: &DomainRule) -> Result<(), NetworkPolicyValidationError> {
    match &rule.action {
        DomainRuleAction::PassThrough => Ok(()),
        DomainRuleAction::Transform(transform) => {
            for match_rule in &transform.match_rules {
                validate_match_rule(match_rule)?;
            }
            Ok(())
        }
        DomainRuleAction::Forward(forward) => {
            for match_rule in &forward.match_rules {
                validate_match_rule(match_rule)?;
            }
            Ok(())
        }
    }
}

fn validate_match_rule(match_rule: &MatchRule) -> Result<(), NetworkPolicyValidationError> {
    if let Some(StringMatcher::Regex(pattern)) = &match_rule.path {
        validate_regex(pattern)?;
    }
    for query_matcher in &match_rule.query_string {
        if let StringMatcher::Regex(pattern) = &query_matcher.value {
            validate_regex(pattern)?;
        }
    }
    for header_matcher in &match_rule.headers {
        if let StringMatcher::Regex(pattern) = &header_matcher.value {
            validate_regex(pattern)?;
        }
    }
    Ok(())
}

fn validate_regex(pattern: &str) -> Result<(), NetworkPolicyValidationError> {
    regex::Regex::new(pattern)
        .map(|_| ())
        .map_err(|source| NetworkPolicyValidationError::InvalidRegex {
            pattern: pattern.to_string(),
            source,
        })
}

#[cfg(test)]
mod tests;
