// TODO: wire `from_yaml` through `serde_yaml` once it is added to Cargo.toml.

use super::errors::RedactionError;
use super::rule::RedactionRule;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionRuleset {
    rules: Vec<RedactionRule>,
}

impl RedactionRuleset {
    #[must_use]
    pub fn builder() -> RedactionRulesetBuilder {
        RedactionRulesetBuilder::default()
    }

    #[must_use]
    pub fn rules(&self) -> &[RedactionRule] {
        &self.rules
    }

    pub fn from_yaml(s: &str) -> Result<Self, RedactionError> {
        let _ = s;
        Err(RedactionError::InvalidYaml("yaml support pending".into()))
    }
}

#[derive(Debug, Default)]
pub struct RedactionRulesetBuilder {
    rules: Vec<RedactionRule>,
}

impl RedactionRulesetBuilder {
    #[must_use]
    pub fn rule(mut self, rule: RedactionRule) -> Self {
        self.rules.push(rule);
        self
    }

    #[must_use]
    pub fn build(self) -> RedactionRuleset {
        RedactionRuleset { rules: self.rules }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redaction::rule::{JsonPath, RedactionAction};

    fn mask_rule(path: &str) -> RedactionRule {
        RedactionRule {
            path: JsonPath::parse(path).expect("valid path"),
            action: RedactionAction::Mask,
        }
    }

    fn hash_rule(path: &str) -> RedactionRule {
        RedactionRule {
            path: JsonPath::parse(path).expect("valid path"),
            action: RedactionAction::Hash,
        }
    }

    #[test]
    fn builder_collects_mask_and_hash_rules() {
        let ruleset = RedactionRuleset::builder()
            .rule(mask_rule("$.foo"))
            .rule(hash_rule("$.bar"))
            .build();

        assert_eq!(
            ruleset.rules(),
            &[
                RedactionRule {
                    path: JsonPath::parse("$.foo").unwrap(),
                    action: RedactionAction::Mask,
                },
                RedactionRule {
                    path: JsonPath::parse("$.bar").unwrap(),
                    action: RedactionAction::Hash,
                },
            ]
        );
    }

    #[test]
    fn from_yaml_returns_pending_error() {
        let yaml = r"
rules:
  - path: $.foo
    action:
      mask: {}
  - path: $.bar
    action:
      hash: {}
";
        assert_eq!(
            RedactionRuleset::from_yaml(yaml).unwrap_err(),
            RedactionError::InvalidYaml("yaml support pending".into())
        );
    }

    #[test]
    fn from_yaml_rejects_unknown_action_via_pending_stub() {
        let yaml = r"
rules:
  - path: $.secret
    action:
      explode: {}
";
        assert_eq!(
            RedactionRuleset::from_yaml(yaml).unwrap_err(),
            RedactionError::InvalidYaml("yaml support pending".into())
        );
    }

    #[test]
    fn from_yaml_rejects_malformed_yaml_via_pending_stub() {
        let yaml = "rules: [not valid yaml structure";
        assert_eq!(
            RedactionRuleset::from_yaml(yaml).unwrap_err(),
            RedactionError::InvalidYaml("yaml support pending".into())
        );
    }

    #[test]
    #[ignore = "requires serde_yaml dependency"]
    fn from_yaml_round_trips_mask_and_hash_rules() {
        let yaml = r"
rules:
  - path: $.foo
    action:
      mask: {}
  - path: $.bar
    action:
      hash: {}
";
        let ruleset = RedactionRuleset::from_yaml(yaml).expect("yaml parsing enabled");
        assert_eq!(
            ruleset.rules(),
            &[
                RedactionRule {
                    path: JsonPath::parse("$.foo").unwrap(),
                    action: RedactionAction::Mask,
                },
                RedactionRule {
                    path: JsonPath::parse("$.bar").unwrap(),
                    action: RedactionAction::Hash,
                },
            ]
        );
    }
}
