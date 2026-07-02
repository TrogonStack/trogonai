//! Fixture schema for `tier2-cel-test` suites.
//!
//! A suite is a YAML or JSON document shaped like:
//!
//! ```yaml
//! suite: my policy bundle
//! bundle: path/to/cel/dir
//! fixtures:
//!   - id: allow-planner-message-send
//!     input:
//!       request: { method: message/send, params: {} }
//!       caller: { id: planner-1 }
//!       agent: { id: planner }
//!       task: {}
//!       headers: { x-tenant-id: acme }
//!     expected_verdict: allow
//! ```
//!
//! `expected_verdict` is either the bare string `allow` or a mapping
//! `{ deny: { rule: <rule-name> } }`, mirroring AGT's `agt test`
//! fixture shape (`expected_verdict: Allow | Deny{rule}`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use a2a_auth_callout::SpiceDbSubject;
use a2a_gateway::policy::{RuleName, Tier2EvaluationContext};
use a2a_nats::{A2aAgentId, A2aTaskId};
use serde::Deserialize;

use crate::method::{RequestMethodError, parse_request_method};

/// A full fixture suite: a named collection of fixtures replayed against a
/// single `.cel` bundle directory.
#[derive(Debug, Deserialize)]
pub struct FixtureSuite {
    pub suite: String,
    /// Bundle directory, resolved relative to the suite file's parent
    /// directory by the caller (kept as the raw path here so this type
    /// stays a pure deserialization target).
    pub bundle: PathBuf,
    pub fixtures: Vec<Fixture>,
}

#[derive(Debug, Deserialize)]
pub struct Fixture {
    pub id: String,
    pub input: FixtureInput,
    pub expected_verdict: ExpectedVerdict,
}

#[derive(Debug, Deserialize)]
pub struct FixtureInput {
    #[serde(default)]
    pub request: RequestInput,
    #[serde(default)]
    pub caller: CallerInput,
    pub agent: AgentInput,
    #[serde(default)]
    pub task: TaskInput,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RequestInput {
    pub method: Option<String>,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Default, Deserialize)]
pub struct CallerInput {
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentInput {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct TaskInput {
    pub id: Option<String>,
}

/// Expected policy verdict for a fixture. Untagged so suites can write
/// either the bare string `allow` or a `deny` mapping without a
/// discriminant field, matching the work item's documented shape
/// `Allow | Deny{rule}`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ExpectedVerdict {
    Allow(AllowTag),
    Deny { deny: DenyDetail },
}

/// Marker so `expected_verdict: allow` deserializes through the same
/// untagged enum as the `deny` mapping variant instead of needing a
/// hand-rolled `Deserialize` impl.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AllowTag {
    Allow,
}

#[derive(Debug, Deserialize)]
pub struct DenyDetail {
    pub rule: String,
}

/// Failure surface for turning a [`FixtureInput`] into a
/// [`Tier2EvaluationContext`]. Each variant names the offending field so a
/// fixture author gets a precise error instead of a generic parse failure.
#[derive(Debug, thiserror::Error)]
pub enum FixtureInputError {
    #[error("fixture input.agent.id is invalid: {0}")]
    AgentId(#[source] a2a_nats::AgentIdError),
    #[error("fixture input.task.id is invalid: {0}")]
    TaskId(#[source] a2a_nats::TaskIdError),
    #[error("fixture input.request.method is invalid: {0}")]
    RequestMethod(#[source] RequestMethodError),
}

impl FixtureInput {
    /// Convert the wire-shaped fixture input into the same
    /// [`Tier2EvaluationContext`] value object the gateway evaluator
    /// consumes at runtime, so a fixture replay exercises identical
    /// binding semantics.
    pub fn to_evaluation_context(&self) -> Result<Tier2EvaluationContext, FixtureInputError> {
        let method = parse_request_method(self.request.method.as_deref().unwrap_or("message/send"))
            .map_err(FixtureInputError::RequestMethod)?;
        let agent_id = A2aAgentId::new(&self.agent.id).map_err(FixtureInputError::AgentId)?;
        let caller_id = self.caller.id.as_deref().map(SpiceDbSubject::new);
        let task_id = self
            .task
            .id
            .as_deref()
            .map(A2aTaskId::new)
            .transpose()
            .map_err(FixtureInputError::TaskId)?;
        Ok(Tier2EvaluationContext::new(
            method,
            self.request.params.clone(),
            caller_id,
            agent_id,
            task_id,
            self.headers.clone(),
        ))
    }
}

/// Parsed expectation, decoupled from the wire [`ExpectedVerdict`] shape so
/// comparison against a [`a2a_gateway::policy::Tier2Decision`] doesn't need
/// to re-inspect the serde enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedOutcome {
    Allow,
    Deny { rule: RuleName },
}

/// Failure surface for turning an [`ExpectedVerdict`] into an
/// [`ExpectedOutcome`]; the only failure mode is an empty `rule` name.
#[derive(Debug, thiserror::Error)]
#[error("fixture expected_verdict.deny.rule is invalid: {0}")]
pub struct ExpectedVerdictError(#[source] a2a_gateway::policy::tier2::rule_name::RuleNameError);

impl ExpectedVerdict {
    pub fn to_outcome(&self) -> Result<ExpectedOutcome, ExpectedVerdictError> {
        match self {
            Self::Allow(AllowTag::Allow) => Ok(ExpectedOutcome::Allow),
            Self::Deny { deny } => {
                let rule = RuleName::new(&deny.rule).map_err(ExpectedVerdictError)?;
                Ok(ExpectedOutcome::Deny { rule })
            }
        }
    }
}

#[cfg(test)]
mod tests;
