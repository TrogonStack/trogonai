//! Fixtures shared by the integration tests. Each test binary uses a subset.
#![allow(dead_code, clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::cell::Cell;
use std::time::Duration;

use trogon_policy_cel::{
    BundleHandle, CelEngine, CelLimits, CelVerdict, CompiledRule, FactName, FactValue, RefusalCause, RequestFacts,
    RuleSet,
};
use trogon_policy_eval::RuleName;
use trogon_std::time::{GetElapsed, GetNow};

pub fn rule_name(value: &str) -> RuleName {
    RuleName::new(value).expect("valid rule name")
}

pub fn fact_name(value: &str) -> FactName {
    FactName::new(value).expect("valid fact name")
}

/// The default limits with a timeout generous enough that a loaded test machine,
/// or a panic that prints a backtrace, cannot trip it. The timeout has its own
/// tests, driven by a clock that moves only when the test says so.
pub fn limits() -> CelLimits {
    let defaults = CelLimits::default();
    CelLimits::new(defaults.max_cost(), Duration::from_secs(30)).expect("non-zero limits")
}

pub fn compile(source: &str) -> Result<CompiledRule, trogon_policy_cel::CompileError> {
    CompiledRule::compile(rule_name("r"), source, &limits())
}

pub fn rule_set(rules: &[(&str, &str)]) -> RuleSet {
    RuleSet::compile(rules.iter().map(|(n, s)| (rule_name(n), *s)), &limits()).expect("rules compile")
}

pub fn engine(rules: &[(&str, &str)]) -> CelEngine {
    CelEngine::new(limits(), BundleHandle::new(rule_set(rules)))
}

/// The facts a gateway might supply: a `request` map and a `caller` map.
pub fn gateway_facts() -> RequestFacts {
    RequestFacts::new()
        .with(
            fact_name("request"),
            FactValue::map([
                ("method", FactValue::from("POST")),
                ("host", FactValue::from("api.example.com")),
                ("port", FactValue::from(443)),
                ("tags", FactValue::list(["a", "b"])),
            ]),
        )
        .with(
            fact_name("caller"),
            FactValue::map([("agent", FactValue::from("a")), ("depth", FactValue::from(2))]),
        )
}

/// The cause of the refusal, or a panic if the verdict passed.
pub fn cause_of(verdict: CelVerdict) -> RefusalCause {
    match verdict {
        CelVerdict::Refused(refusal) => refusal.cause().clone(),
        CelVerdict::Pass => panic!("expected a refusal"),
    }
}

/// The rule that refused, or a panic if the verdict passed.
pub fn refusing_rule(verdict: &CelVerdict) -> String {
    match verdict {
        CelVerdict::Refused(refusal) => refusal.rule().as_str().to_owned(),
        CelVerdict::Pass => panic!("expected a refusal"),
    }
}

/// A clock that moves forward by a fixed step every time it is asked how long
/// has passed.
pub struct SteppingClock {
    step: Duration,
    elapsed: Cell<Duration>,
}

impl SteppingClock {
    pub fn new(step: Duration) -> Self {
        Self {
            step,
            elapsed: Cell::new(Duration::ZERO),
        }
    }
}

impl GetNow for SteppingClock {
    type Instant = Duration;

    fn now(&self) -> Duration {
        self.elapsed.get()
    }
}

impl GetElapsed for SteppingClock {
    fn elapsed(&self, since: Duration) -> Duration {
        let now = self.elapsed.get() + self.step;
        self.elapsed.set(now);
        now - since
    }
}
