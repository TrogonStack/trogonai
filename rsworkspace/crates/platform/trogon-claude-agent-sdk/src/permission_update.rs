use crate::{JsonObject, PermissionMode, ToolName, constants};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Where a permission update is stored.
///
/// Both upstream SDKs accept any destination string, so a destination this
/// crate does not know yet is kept verbatim in
/// [`PermissionUpdateDestination::Unknown`].
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionUpdateDestination {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    Session,
    CliArg,
    #[serde(untagged)]
    Unknown(String),
}

/// How a permission rule applies.
///
/// Both upstream SDKs accept any behavior string, so a behavior this crate does
/// not know yet is kept verbatim in [`PermissionBehavior::Unknown`].
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct PermissionRuleValue {
    pub tool_name: ToolName,
    pub rule_content: Option<String>,
}

#[derive(Clone, Debug)]
pub enum PermissionUpdate {
    AddRules {
        rules: Vec<PermissionRuleValue>,
        behavior: PermissionBehavior,
        destination: PermissionUpdateDestination,
    },
    ReplaceRules {
        rules: Vec<PermissionRuleValue>,
        behavior: PermissionBehavior,
        destination: PermissionUpdateDestination,
    },
    RemoveRules {
        rules: Vec<PermissionRuleValue>,
        behavior: PermissionBehavior,
        destination: PermissionUpdateDestination,
    },
    SetMode {
        mode: PermissionMode,
        destination: PermissionUpdateDestination,
    },
    AddDirectories {
        directories: Vec<PathBuf>,
        destination: PermissionUpdateDestination,
    },
    RemoveDirectories {
        directories: Vec<PathBuf>,
        destination: PermissionUpdateDestination,
    },
    /// An update `type` this crate does not know yet, kept as the raw object.
    Unknown(JsonObject),
}

#[derive(Debug, thiserror::Error)]
pub enum PermissionUpdateError {
    #[error("malformed permission update")]
    Json(#[from] serde_json::Error),
}

#[derive(Deserialize, Serialize)]
struct RuleWire {
    #[serde(rename = "toolName")]
    tool_name: String,
    #[serde(rename = "ruleContent", skip_serializing_if = "Option::is_none")]
    rule_content: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum UpdateWire {
    AddRules {
        rules: Vec<RuleWire>,
        behavior: PermissionBehavior,
        destination: PermissionUpdateDestination,
    },
    ReplaceRules {
        rules: Vec<RuleWire>,
        behavior: PermissionBehavior,
        destination: PermissionUpdateDestination,
    },
    RemoveRules {
        rules: Vec<RuleWire>,
        behavior: PermissionBehavior,
        destination: PermissionUpdateDestination,
    },
    SetMode {
        mode: String,
        destination: PermissionUpdateDestination,
    },
    AddDirectories {
        directories: Vec<PathBuf>,
        destination: PermissionUpdateDestination,
    },
    RemoveDirectories {
        directories: Vec<PathBuf>,
        destination: PermissionUpdateDestination,
    },
}

impl PermissionUpdate {
    pub fn from_wire_json(value: JsonObject) -> Result<Self, PermissionUpdateError> {
        if let Some(kind) = value.get("type").and_then(Value::as_str)
            && !constants::KNOWN_UPDATE_TYPES.contains(&kind)
        {
            return Ok(Self::Unknown(value));
        }
        let wire: UpdateWire = serde_json::from_value(value.into())?;
        let rules = |values: Vec<RuleWire>| {
            values
                .into_iter()
                .map(|value| PermissionRuleValue {
                    tool_name: ToolName::from(value.tool_name),
                    rule_content: value.rule_content,
                })
                .collect::<Vec<_>>()
        };
        Ok(match wire {
            UpdateWire::AddRules {
                rules: values,
                behavior,
                destination,
            } => Self::AddRules {
                rules: rules(values),
                behavior,
                destination,
            },
            UpdateWire::ReplaceRules {
                rules: values,
                behavior,
                destination,
            } => Self::ReplaceRules {
                rules: rules(values),
                behavior,
                destination,
            },
            UpdateWire::RemoveRules {
                rules: values,
                behavior,
                destination,
            } => Self::RemoveRules {
                rules: rules(values),
                behavior,
                destination,
            },
            UpdateWire::SetMode { mode, destination } => Self::SetMode {
                mode: PermissionMode::from(mode),
                destination,
            },
            UpdateWire::AddDirectories {
                directories,
                destination,
            } => Self::AddDirectories {
                directories,
                destination,
            },
            UpdateWire::RemoveDirectories {
                directories,
                destination,
            } => Self::RemoveDirectories {
                directories,
                destination,
            },
        })
    }

    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let rule_values = |rules: &[PermissionRuleValue]| {
            rules
                .iter()
                .map(|rule| RuleWire {
                    tool_name: rule.tool_name.as_str().to_owned(),
                    rule_content: rule.rule_content.clone(),
                })
                .collect()
        };
        let wire = match self {
            Self::Unknown(value) => return Ok(Value::from(value.clone())),
            Self::AddRules {
                rules,
                behavior,
                destination,
            } => UpdateWire::AddRules {
                rules: rule_values(rules),
                behavior: behavior.clone(),
                destination: destination.clone(),
            },
            Self::ReplaceRules {
                rules,
                behavior,
                destination,
            } => UpdateWire::ReplaceRules {
                rules: rule_values(rules),
                behavior: behavior.clone(),
                destination: destination.clone(),
            },
            Self::RemoveRules {
                rules,
                behavior,
                destination,
            } => UpdateWire::RemoveRules {
                rules: rule_values(rules),
                behavior: behavior.clone(),
                destination: destination.clone(),
            },
            Self::SetMode { mode, destination } => UpdateWire::SetMode {
                mode: mode.as_str().to_owned(),
                destination: destination.clone(),
            },
            Self::AddDirectories {
                directories,
                destination,
            } => UpdateWire::AddDirectories {
                directories: directories.clone(),
                destination: destination.clone(),
            },
            Self::RemoveDirectories {
                directories,
                destination,
            } => UpdateWire::RemoveDirectories {
                directories: directories.clone(),
                destination: destination.clone(),
            },
        };
        serde_json::to_value(wire)
    }

    /// Python's `PermissionUpdate.to_dict()`: `type`, then `destination`, then the variant's fields, with
    /// `ruleContent` written as `null` when a rule has none.
    pub(crate) fn to_python_wire_json(&self) -> Result<Value, serde_json::Error> {
        let (kind, destination, fields) = match self {
            Self::Unknown(value) => return Ok(Value::from(value.clone())),
            Self::AddRules {
                rules,
                behavior,
                destination,
            } => ("addRules", destination, python_rules(rules, behavior)?),
            Self::ReplaceRules {
                rules,
                behavior,
                destination,
            } => ("replaceRules", destination, python_rules(rules, behavior)?),
            Self::RemoveRules {
                rules,
                behavior,
                destination,
            } => ("removeRules", destination, python_rules(rules, behavior)?),
            Self::SetMode { mode, destination } => ("setMode", destination, vec![("mode", Value::from(mode.as_str()))]),
            Self::AddDirectories {
                directories,
                destination,
            } => (
                "addDirectories",
                destination,
                vec![("directories", serde_json::to_value(directories)?)],
            ),
            Self::RemoveDirectories {
                directories,
                destination,
            } => (
                "removeDirectories",
                destination,
                vec![("directories", serde_json::to_value(directories)?)],
            ),
        };
        let mut object = JsonObject::new();
        object.insert("type".to_owned(), Value::from(kind));
        object.insert("destination".to_owned(), serde_json::to_value(destination)?);
        object.extend(fields.into_iter().map(|(key, value)| (key.to_owned(), value)));
        Ok(Value::Object(object))
    }
}

/// The `rules` and `behavior` fields of Python's rule updates.
fn python_rules(
    rules: &[PermissionRuleValue],
    behavior: &PermissionBehavior,
) -> Result<Vec<(&'static str, Value)>, serde_json::Error> {
    let rules = rules
        .iter()
        .map(|rule| {
            let mut value = JsonObject::new();
            value.insert("toolName".to_owned(), Value::from(rule.tool_name.as_str()));
            value.insert(
                "ruleContent".to_owned(),
                rule.rule_content.clone().map_or(Value::Null, Value::from),
            );
            Value::Object(value)
        })
        .collect();
    Ok(vec![
        ("rules", Value::Array(rules)),
        ("behavior", serde_json::to_value(behavior)?),
    ])
}

#[cfg(test)]
#[path = "permission_update_tests.rs"]
mod tests;
