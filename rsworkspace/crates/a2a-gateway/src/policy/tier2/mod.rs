pub mod rule_name;

use std::collections::BTreeMap;

use a2a_nats::A2aAgentId;

use rule_name::RuleName;

#[derive(Clone, Debug)]
pub struct Tier2EvaluationContext {
    request_method: String,
    request_params: serde_json::Value,
    caller_id: Option<String>,
    agent_id: A2aAgentId,
    task_id: Option<String>,
    headers: BTreeMap<String, String>,
}

impl Tier2EvaluationContext {
    pub fn new(
        request_method: impl Into<String>,
        request_params: serde_json::Value,
        caller_id: Option<String>,
        agent_id: A2aAgentId,
        task_id: Option<String>,
        headers: BTreeMap<String, String>,
    ) -> Self {
        Self {
            request_method: request_method.into(),
            request_params,
            caller_id,
            agent_id,
            task_id,
            headers,
        }
    }

    pub fn request_method(&self) -> &str {
        &self.request_method
    }

    pub fn request_params(&self) -> &serde_json::Value {
        &self.request_params
    }

    pub fn caller_id(&self) -> Option<&str> {
        self.caller_id.as_deref()
    }

    pub fn agent_id(&self) -> &A2aAgentId {
        &self.agent_id
    }

    pub fn task_id(&self) -> Option<&str> {
        self.task_id.as_deref()
    }

    pub fn headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Tier2Decision {
    Allow,
    Deny { rule: RuleName },
}

impl Tier2Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

pub trait Tier2CelEvaluator: Send + Sync {
    fn evaluate(&self, ctx: &Tier2EvaluationContext) -> Tier2Decision;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct NoopTier2Evaluator;

impl Tier2CelEvaluator for NoopTier2Evaluator {
    fn evaluate(&self, _ctx: &Tier2EvaluationContext) -> Tier2Decision {
        Tier2Decision::Allow
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct DenyAllTier2Evaluator;

impl Tier2CelEvaluator for DenyAllTier2Evaluator {
    fn evaluate(&self, _ctx: &Tier2EvaluationContext) -> Tier2Decision {
        Tier2Decision::Deny {
            rule: RuleName::evaluation_error(),
        }
    }
}
