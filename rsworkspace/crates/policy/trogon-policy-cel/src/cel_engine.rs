use std::panic::{AssertUnwindSafe, catch_unwind};

use cel_interpreter::{Context, Value};
use trogon_policy_eval::{Narrowing, RequestRuleEngine};
use trogon_std::time::{GetElapsed, GetNow, SystemClock};

use crate::bundle_handle::BundleHandle;
use crate::cel_limits::CelLimits;
use crate::cel_refusal::CelRefusal;
use crate::cel_verdict::CelVerdict;
use crate::compiled_rule::CompiledRule;
use crate::refusal_cause::RefusalCause;
use crate::request_facts::RequestFacts;
use crate::runtime_error_kind::RuntimeErrorKind;

/// Evaluates the request rules in force against the facts of one request.
///
/// The engine can only narrow. Its answer is a [`CelVerdict`], which has a
/// refusal and a pass that means no rule refused, and through
/// [`RequestRuleEngine`] it reaches the evaluator as a [`Narrowing`] with the
/// same two arms. Every way a rule can fail to answer is a refusal: a result
/// that is not a boolean, a runtime error, a missing field, an unknown name, an
/// interpreter panic, facts over the input limits, and evaluation that outlasts
/// the timeout. An empty rule set refuses nothing.
///
/// Evaluation reads the [`BundleHandle`] and the facts, and nothing else. It
/// never touches the filesystem, so a bundle reload cannot slow a request.
///
/// # The timeout
///
/// The interpreter cannot be interrupted, so the timeout is checked after each
/// rule against the injected clock and cannot stop a rule that has already
/// started. A rule that runs long refuses its request, and so does every rule
/// after it. What keeps a single rule short is the cost limit, enforced when
/// the rule is compiled.
#[derive(Debug, Clone)]
pub struct CelEngine<K = SystemClock> {
    bundle: BundleHandle,
    limits: CelLimits,
    clock: K,
}

impl CelEngine<SystemClock> {
    /// Builds an engine that reads the system clock.
    pub fn new(limits: CelLimits, bundle: BundleHandle) -> Self {
        Self::with_clock(limits, bundle, SystemClock)
    }
}

impl<K> CelEngine<K> {
    /// Builds an engine that reads this clock.
    pub fn with_clock(limits: CelLimits, bundle: BundleHandle, clock: K) -> Self {
        Self { bundle, limits, clock }
    }

    /// The limits the engine enforces.
    pub fn limits(&self) -> &CelLimits {
        &self.limits
    }
}

impl<K: GetNow + GetElapsed> CelEngine<K> {
    /// Runs every rule in force against the facts and reports the first, by
    /// name, that refuses.
    pub fn evaluate(&self, facts: &RequestFacts) -> CelVerdict {
        let rules = self.bundle.current();
        let Some(first) = rules.rules().first() else {
            return CelVerdict::Pass;
        };
        let started = self.clock.now();
        if let Err(rejected) = facts.check(&self.limits) {
            return refused(first, RefusalCause::FactsRejected(rejected));
        }
        let context = facts.to_context();
        for rule in rules.rules() {
            let outcome = run(rule, &context);
            if self.clock.elapsed(started) > self.limits.timeout() {
                return refused(
                    rule,
                    RefusalCause::TimedOut {
                        limit: self.limits.timeout(),
                    },
                );
            }
            if let Err(cause) = outcome {
                return refused(rule, cause);
            }
        }
        CelVerdict::Pass
    }
}

impl<K: GetNow + GetElapsed> RequestRuleEngine for CelEngine<K> {
    type Facts = RequestFacts;

    fn narrow(&self, facts: &RequestFacts) -> Narrowing {
        let verdict = self.evaluate(facts);
        if let CelVerdict::Refused(refusal) = &verdict {
            if refusal.cause().is_ordinary() {
                tracing::debug!(rule = %refusal.rule(), "request rule refused the request");
            } else {
                tracing::warn!(rule = %refusal.rule(), cause = %refusal.cause(), "request rule could not answer, refusing");
            }
        }
        verdict.into_narrowing()
    }
}

fn refused(rule: &CompiledRule, cause: RefusalCause) -> CelVerdict {
    CelVerdict::Refused(CelRefusal::new(rule.name().clone(), cause))
}

/// Runs one rule. The interpreter panics on a few inputs it does not support,
/// such as looping over a value that is neither a list nor a map, so a panic is
/// caught and reported as the rule failing.
fn run(rule: &CompiledRule, context: &Context<'_>) -> Result<(), RefusalCause> {
    match catch_unwind(AssertUnwindSafe(|| context.resolve(rule.expression()))) {
        Err(_) => Err(RefusalCause::Panicked),
        Ok(Err(error)) => Err(RefusalCause::Runtime(RuntimeErrorKind::of(&error))),
        Ok(Ok(Value::Bool(true))) => Ok(()),
        Ok(Ok(Value::Bool(false))) => Err(RefusalCause::False),
        Ok(Ok(other)) => Err(RefusalCause::NotBoolean { found: kind_of(&other) }),
    }
}

// `run` settles a boolean before it asks, so no arm names one.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::List(_) => "a list",
        Value::Map(_) => "a map",
        Value::Function(..) => "a function",
        Value::Int(_) => "an integer",
        Value::UInt(_) => "an unsigned integer",
        Value::Float(_) => "a float",
        Value::String(_) => "a string",
        Value::Bytes(_) => "bytes",
        Value::Null => "null",
        _ => "a value of another type",
    }
}
