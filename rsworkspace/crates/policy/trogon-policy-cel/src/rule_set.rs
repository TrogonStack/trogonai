use trogon_policy_eval::RuleName;

use crate::cel_limits::CelLimits;
use crate::compiled_rule::CompiledRule;
use crate::constants::MAX_RULES_PER_SET;
use crate::rule_set_error::RuleSetError;

/// The compiled rules a request must satisfy, in a fixed order.
///
/// Rules are kept sorted by name, so the rule a refusal names does not depend
/// on the order the rules were supplied in: it is the first one, by name, that
/// refuses. An empty set refuses nothing. That is the default for a deployment
/// that has authored no request rules, and it is not a way around the bindings,
/// which have to admit the request first.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<CompiledRule>,
}

impl RuleSet {
    /// The set with no rules.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Compiles every `(name, source)` pair, or says which one failed.
    pub fn compile<S: AsRef<str>>(
        sources: impl IntoIterator<Item = (RuleName, S)>,
        limits: &CelLimits,
    ) -> Result<Self, RuleSetError> {
        let mut rules = Vec::new();
        for (name, source) in sources {
            let rule = CompiledRule::compile(name.clone(), source.as_ref(), limits)
                .map_err(|source| RuleSetError::Rule { name, source })?;
            rules.push(rule);
        }
        Self::from_rules(rules)
    }

    /// Builds a set from rules compiled elsewhere.
    pub fn from_rules(mut rules: Vec<CompiledRule>) -> Result<Self, RuleSetError> {
        if rules.len() > MAX_RULES_PER_SET {
            return Err(RuleSetError::TooMany {
                actual: rules.len(),
                limit: MAX_RULES_PER_SET,
            });
        }
        rules.sort_by(|a, b| a.name().cmp(b.name()));
        if let Some(pair) = rules.windows(2).find(|pair| pair[0].name() == pair[1].name()) {
            return Err(RuleSetError::Duplicate {
                name: pair[0].name().clone(),
            });
        }
        Ok(Self { rules })
    }

    /// How many rules there are.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Returns whether there are no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The rules, in the order they are evaluated.
    pub fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }
}
