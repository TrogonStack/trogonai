pub mod rule_name;

use std::collections::BTreeMap;

use a2a_auth_callout::SpiceDbSubject;
use a2a_nats::server::A2aMethod;
use a2a_nats::{A2aAgentId, A2aTaskId};

use rule_name::RuleName;

/// Input to a Tier-2 CEL rule evaluation.
///
/// Every identity-typed boundary input — the request method, the caller
/// subject, the originating agent, the task id — flows in as its
/// validated value object rather than a raw string. Storing them typed
/// means an empty `caller_id` or an invalid `task_id` can't reach the
/// evaluator at all, and the audit subject can serialize each field
/// without re-parsing.
///
/// `headers` is kept as a `BTreeMap<String, String>` because it's a
/// pass-through of arbitrary wire-protocol headers — those don't have
/// a canonical Trogon value-object form.
#[derive(Clone, Debug)]
pub struct Tier2EvaluationContext {
    request_method: A2aMethod,
    request_params: serde_json::Value,
    caller_id: Option<SpiceDbSubject>,
    agent_id: A2aAgentId,
    task_id: Option<A2aTaskId>,
    headers: BTreeMap<String, String>,
}

impl Tier2EvaluationContext {
    pub fn new(
        request_method: A2aMethod,
        request_params: serde_json::Value,
        caller_id: Option<SpiceDbSubject>,
        agent_id: A2aAgentId,
        task_id: Option<A2aTaskId>,
        headers: BTreeMap<String, String>,
    ) -> Self {
        Self {
            request_method,
            request_params,
            caller_id,
            agent_id,
            task_id,
            headers,
        }
    }

    pub fn request_method(&self) -> &A2aMethod {
        &self.request_method
    }

    pub fn request_params(&self) -> &serde_json::Value {
        &self.request_params
    }

    pub fn caller_id(&self) -> Option<&SpiceDbSubject> {
        self.caller_id.as_ref()
    }

    pub fn agent_id(&self) -> &A2aAgentId {
        &self.agent_id
    }

    pub fn task_id(&self) -> Option<&A2aTaskId> {
        self.task_id.as_ref()
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
