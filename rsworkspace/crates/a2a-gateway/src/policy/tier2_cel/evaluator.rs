use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use a2a_nats::A2aAgentId;
use async_nats::HeaderMap;
use cel_interpreter::{to_value, Context, Value};
use tracing::warn;

use crate::policy::error::Tier2EvalError;

use super::bundle::{CelProgramHandle, Tier2CompiledBundle};
use crate::policy::tier2::rule_name::RuleName;
use crate::policy::tier2::{Tier2CelEvaluator, Tier2Decision, Tier2EvaluationContext};

pub trait CelEngine: Send + Sync {
    fn evaluate_bool(
        &self,
        rule: &RuleName,
        program: &CelProgramHandle,
        ctx: &Tier2EvaluationContext,
    ) -> Result<bool, Tier2EvalError>;
}

#[derive(Default)]
pub struct CelInterpreterEngine;

impl CelEngine for CelInterpreterEngine {
    fn evaluate_bool(
        &self,
        _rule: &RuleName,
        program: &CelProgramHandle,
        ctx: &Tier2EvaluationContext,
    ) -> Result<bool, Tier2EvalError> {
        let mut cel_ctx = Context::default();
        bind_evaluation_context(&mut cel_ctx, ctx)?;
        let value = program
            .program()
            .execute(&cel_ctx)
            .map_err(|err| Tier2EvalError::new(format!("CEL execution failed: {err}")))?;
        match value {
            Value::Bool(result) => Ok(result),
            other => Err(Tier2EvalError::new(format!(
                "CEL rule must return bool, got {other:?}"
            ))),
        }
    }
}

pub struct RealTier2CelEvaluator {
    bundle: Mutex<Tier2CompiledBundle>,
    engine: Arc<dyn CelEngine>,
}

impl RealTier2CelEvaluator {
    pub fn new(bundle: Tier2CompiledBundle) -> Self {
        Self {
            bundle: Mutex::new(bundle),
            engine: Arc::new(CelInterpreterEngine),
        }
    }

    pub fn with_engine(bundle: Tier2CompiledBundle, engine: Arc<dyn CelEngine>) -> Self {
        Self {
            bundle: Mutex::new(bundle),
            engine,
        }
    }
}

impl Tier2CelEvaluator for RealTier2CelEvaluator {
    fn evaluate(&self, ctx: &Tier2EvaluationContext) -> Tier2Decision {
        let mut bundle = match self.bundle.lock() {
            Ok(guard) => guard,
            Err(_) => {
                warn!("tier-2 bundle lock poisoned; denying request");
                return Tier2Decision::Deny {
                    rule: RuleName::evaluation_error(),
                };
            }
        };

        if let Err(err) = bundle.refresh_if_stale() {
            warn!(error = %err, "tier-2 bundle refresh failed; denying request");
            return Tier2Decision::Deny {
                rule: RuleName::evaluation_error(),
            };
        }

        for (rule, program) in bundle.rules() {
            match self.engine.evaluate_bool(rule, program, ctx) {
                Ok(true) => {}
                Ok(false) => return Tier2Decision::Deny { rule: rule.clone() },
                Err(err) => {
                    warn!(rule = %rule, error = %err, "tier-2 CEL rule evaluation failed; denying request");
                    return Tier2Decision::Deny {
                        rule: RuleName::evaluation_error(),
                    };
                }
            }
        }

        Tier2Decision::Allow
    }
}

fn bind_evaluation_context(
    cel_ctx: &mut Context,
    ctx: &Tier2EvaluationContext,
) -> Result<(), Tier2EvalError> {
    let request = to_value(serde_json::json!({
        "method": ctx.request_method(),
        "params": ctx.request_params(),
    }))
    .map_err(|err| Tier2EvalError::new(format!("request binding failed: {err}")))?;
    cel_ctx.add_variable_from_value("request", request);

    let caller = to_value(serde_json::json!({
        "id": ctx.caller_id(),
    }))
    .map_err(|err| Tier2EvalError::new(format!("caller binding failed: {err}")))?;
    cel_ctx.add_variable_from_value("caller", caller);

    let agent = to_value(serde_json::json!({
        "id": ctx.agent_id().as_str(),
    }))
    .map_err(|err| Tier2EvalError::new(format!("agent binding failed: {err}")))?;
    cel_ctx.add_variable_from_value("agent", agent);

    let task = to_value(serde_json::json!({
        "id": ctx.task_id(),
    }))
    .map_err(|err| Tier2EvalError::new(format!("task binding failed: {err}")))?;
    cel_ctx.add_variable_from_value("task", task);

    let headers: BTreeMap<String, String> = ctx
        .headers()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let headers_value = to_value(headers)
        .map_err(|err| Tier2EvalError::new(format!("headers binding failed: {err}")))?;
    cel_ctx.add_variable_from_value("headers", headers_value);

    Ok(())
}

pub fn tier2_evaluation_context_from_ingress(
    method_slashes: &str,
    agent_id: &A2aAgentId,
    caller_id: Option<&str>,
    headers: &HeaderMap,
    payload: &[u8],
) -> Tier2EvaluationContext {
    let (params, task_id) = parse_json_rpc_params(payload);
    let header_map = headers_to_map(headers);
    Tier2EvaluationContext::new(
        method_slashes,
        params,
        caller_id.map(str::to_owned),
        agent_id.clone(),
        task_id,
        header_map,
    )
}

fn parse_json_rpc_params(payload: &[u8]) -> (serde_json::Value, Option<String>) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return (serde_json::Value::Null, None);
    };
    let params = value.get("params").cloned().unwrap_or(serde_json::Value::Null);
    let task_id = params
        .get("taskId")
        .or_else(|| params.get("task_id"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    (params, task_id)
}

fn headers_to_map(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (name, values) in headers.iter() {
        if let Some(value) = values.first()
            && let Ok(text) = std::str::from_utf8(value.as_ref())
        {
            map.insert(name.to_string(), text.to_owned());
        }
    }
    map
}

#[cfg(test)]
pub(crate) struct MockCelEngine {
    outcomes: BTreeMap<RuleName, Result<bool, Tier2EvalError>>,
}

#[cfg(test)]
impl MockCelEngine {
    pub fn new(outcomes: BTreeMap<RuleName, Result<bool, Tier2EvalError>>) -> Self {
        Self { outcomes }
    }
}

#[cfg(test)]
impl CelEngine for MockCelEngine {
    fn evaluate_bool(
        &self,
        rule: &RuleName,
        _program: &CelProgramHandle,
        _ctx: &Tier2EvaluationContext,
    ) -> Result<bool, Tier2EvalError> {
        match self.outcomes.get(rule) {
            Some(Ok(result)) => Ok(*result),
            Some(Err(err)) => Err(Tier2EvalError::new(err.to_string())),
            None => Ok(true),
        }
    }
}
