use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Normalized capability tags for LLM runners registered in the agent registry.
///
/// These values describe what a runner advertises in its registry record. They
/// are **descriptive metadata only** and MUST NOT be used to gate feature
/// availability or route user requests — use the model catalog and explicit
/// feature checks for that instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerCapability {
    Chat,
    CodeEdit,
    Explore,
    Plan,
}

impl RunnerCapability {
    pub const ALL: &[Self] = &[Self::Chat, Self::CodeEdit, Self::Explore, Self::Plan];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::CodeEdit => "code_edit",
            Self::Explore => "explore",
            Self::Plan => "plan",
        }
    }

    pub fn to_strings(caps: &[Self]) -> Vec<String> {
        caps.iter().map(|c| c.as_str().to_string()).collect()
    }
}

impl FromStr for RunnerCapability {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "chat" => Ok(Self::Chat),
            "code_edit" => Ok(Self::CodeEdit),
            "explore" => Ok(Self::Explore),
            "plan" => Ok(Self::Plan),
            _ => Err(()),
        }
    }
}

/// Expected capability tags for the known LLM runners that self-register at startup.
pub fn expected_runner_capabilities(agent_type: &str) -> Option<&'static [RunnerCapability]> {
    match agent_type {
        "claude" | "codex" => Some(&[RunnerCapability::Chat, RunnerCapability::CodeEdit]),
        "openrouter" | "xai" => Some(&[
            RunnerCapability::Chat,
            RunnerCapability::Explore,
            RunnerCapability::Plan,
        ]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_capability_round_trips_through_serde() {
        for cap in RunnerCapability::ALL {
            let json = serde_json::to_string(cap).unwrap();
            let restored: RunnerCapability = serde_json::from_str(&json).unwrap();
            assert_eq!(*cap, restored);
        }
    }

    #[test]
    fn runner_capability_as_str_matches_from_str() {
        for cap in RunnerCapability::ALL {
            assert_eq!(RunnerCapability::from_str(cap.as_str()), Ok(*cap));
        }
    }

    #[test]
    fn known_runners_map_to_expected_capabilities() {
        let cases = [
            ("claude", &["chat", "code_edit"][..]),
            ("codex", &["chat", "code_edit"]),
            ("xai", &["chat", "explore", "plan"]),
            ("openrouter", &["chat", "explore", "plan"]),
        ];
        for (agent_type, expected) in cases {
            let caps = expected_runner_capabilities(agent_type).expect(agent_type);
            let got: Vec<&str> = caps.iter().map(|c| c.as_str()).collect();
            assert_eq!(got, expected, "unexpected capabilities for {agent_type}");
        }
    }

    #[test]
    fn to_strings_produces_registry_wire_format() {
        let caps = expected_runner_capabilities("xai").unwrap();
        assert_eq!(
            RunnerCapability::to_strings(caps),
            vec!["chat".to_string(), "explore".to_string(), "plan".to_string(),]
        );
    }
}
