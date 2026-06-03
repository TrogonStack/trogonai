use serde::Deserialize;

use super::errors::RedactionError;
use super::rule::{JsonPath, RedactionAction, RedactionRule};

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
        let raw: RawRuleset =
            serde_norway::from_str(s).map_err(|e| RedactionError::InvalidYaml(e.to_string()))?;
        let mut rules = Vec::with_capacity(raw.rules.len());
        for raw_rule in raw.rules {
            rules.push(RedactionRule {
                path: JsonPath::parse(&raw_rule.path)?,
                action: raw_rule.action.into_action()?,
            });
        }
        Ok(Self { rules })
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

#[derive(Debug, Deserialize)]
struct RawRuleset {
    #[serde(default)]
    rules: Vec<RawRule>,
}

#[derive(Debug, Deserialize)]
struct RawRule {
    path: String,
    action: RawAction,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawAction {
    Bare(String),
    Detailed(DetailedAction),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DetailedAction {
    Replace { value: String },
    RegexReplace { pattern: String, replacement: String },
}

impl RawAction {
    fn into_action(self) -> Result<RedactionAction, RedactionError> {
        match self {
            Self::Bare(name) => match name.as_str() {
                "mask" => Ok(RedactionAction::Mask),
                "hash" => Ok(RedactionAction::Hash),
                "drop" => Ok(RedactionAction::Drop),
                other => Err(RedactionError::UnknownAction(other.to_string())),
            },
            Self::Detailed(DetailedAction::Replace { value }) => Ok(RedactionAction::Replace(value)),
            Self::Detailed(DetailedAction::RegexReplace { pattern, replacement }) => {
                Ok(RedactionAction::RegexReplace { pattern, replacement })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn from_yaml_round_trips_mask_and_hash_rules() {
        let yaml = r"
rules:
  - path: $.foo
    action: mask
  - path: $.bar
    action: hash
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

    #[test]
    fn from_yaml_parses_replace_and_regex_replace() {
        let yaml = r#"
rules:
  - path: $.user.email
    action:
      replace:
        value: "[redacted]"
  - path: $.user.ssn
    action:
      regex_replace:
        pattern: "\\d{3}-\\d{2}-\\d{4}"
        replacement: "***-**-****"
"#;
        let ruleset = RedactionRuleset::from_yaml(yaml).expect("yaml parsing enabled");
        assert_eq!(ruleset.rules().len(), 2);
        assert_eq!(
            ruleset.rules()[0].action,
            RedactionAction::Replace("[redacted]".to_string())
        );
        assert_eq!(
            ruleset.rules()[1].action,
            RedactionAction::RegexReplace {
                pattern: "\\d{3}-\\d{2}-\\d{4}".into(),
                replacement: "***-**-****".into(),
            }
        );
    }

    #[test]
    fn from_yaml_rejects_unknown_action() {
        let yaml = r"
rules:
  - path: $.secret
    action: explode
";
        let err = RedactionRuleset::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, RedactionError::UnknownAction(_)), "got {err:?}");
    }

    #[test]
    fn from_yaml_rejects_malformed_yaml() {
        let yaml = "rules: [not valid yaml structure";
        let err = RedactionRuleset::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, RedactionError::InvalidYaml(_)), "got {err:?}");
    }

    #[test]
    fn from_yaml_rejects_invalid_path() {
        let yaml = r"
rules:
  - path: invalid_no_dollar
    action: mask
";
        let err = RedactionRuleset::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, RedactionError::InvalidPath(_)), "got {err:?}");
    }
}
