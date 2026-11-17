#![allow(
    dead_code,
    reason = "Each golden test reads a different part of the captured fixtures."
)]

//! Reads goldens that were captured from a live run of the upstream SDKs (`sdk.mjs` 0.3.323 and
//! `claude_agent_sdk` 0.2.181) against a scripted CLI. Every value in a fixture is what the upstream SDK wrote or
//! returned, never a hand-written expectation.

use serde_json::Value;

type Failure = Box<dyn std::error::Error>;

pub struct Captured(Value);

impl Captured {
    /// Parses the text of one fixture file.
    pub fn parse(text: &str) -> Result<Self, Failure> {
        Ok(Self(serde_json::from_str(text)?))
    }

    /// One captured scenario's field, such as `responses`, `requests`, `argv`, `env`, `order` or `seen`.
    pub fn field(&self, scenario: &str, field: &str) -> Result<&Value, Failure> {
        self.0["scenarios"][scenario]
            .get(field)
            .ok_or_else(|| format!("fixture has no `{field}` for `{scenario}`").into())
    }

    /// A captured list of strings.
    pub fn strings(&self, scenario: &str, field: &str) -> Result<Vec<String>, Failure> {
        self.field(scenario, field)?
            .as_array()
            .ok_or_else(|| format!("`{scenario}.{field}` is not a list"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("`{scenario}.{field}` holds a non-string").into())
            })
            .collect()
    }
}
