//! CEL request rules for the policy evaluator.
//!
//! Request rules are the part of
//! [ADR#0071](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
//! that looks at one request instead of at bindings. They can only narrow:
//! [`CelEngine`] answers a [`CelVerdict`] with a pass that means "no rule
//! refused" and a refusal, and nothing that grants. A request is admitted when
//! the bindings admit it and every rule passes, which is the composition the
//! evaluator performs when it is given this engine as a
//! [`RequestRuleEngine`](trogon_policy_eval::RequestRuleEngine).
//!
//! This crate depends on the evaluator and the evaluator does not depend on it.
//!
//! # Rules
//!
//! A rule is one CEL expression over named facts. A [`RequestFacts`] holds the
//! facts of one request as plain values under names the host chooses, so the
//! crate knows nothing about gateways or agents. A request passes a rule when
//! the expression is the boolean `true`. Anything else refuses it:
//!
//! - `false`, or a value that is not a boolean;
//! - a runtime error, such as a type mismatch or division by zero;
//! - a name that is not a fact, or a field a fact does not have;
//! - an interpreter panic, which is caught;
//! - facts above the input limits, or evaluation past the timeout.
//!
//! A [`RuleSet`] with no rules passes every request.
//!
//! # Compile once, bounded in advance
//!
//! [`CompiledRule::compile`] parses a rule once and rejects what could never
//! run well: source that is too long or nested too deep, constructs the
//! interpreter does not support, and a rule whose worst-case cost over facts at
//! the [`CelLimits`] is above the cost limit. The interpreter has no cost hook
//! and no cancellation, so the cost is a static upper bound rather than a
//! counter, and the timeout is checked between rules. See [`CelEngine`].
//!
//! # Reload is off the request path
//!
//! [`BundleLoader`] reads a directory of `<name>.cel` files, compiles the whole
//! bundle, and swaps it into a [`BundleHandle`] only if every rule is valid.
//! [`CelEngine::evaluate`] reads the handle and the facts and touches no file.
//! The crate starts no task of its own: the host decides when to reload.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod constants;

mod bundle_error;
mod bundle_handle;
mod bundle_loader;
mod cel_engine;
mod cel_limits;
mod cel_refusal;
mod cel_verdict;
mod compile_error;
mod compiled_rule;
mod cost_model;
mod fact_name;
mod fact_value;
mod facts_limit_error;
mod lexical_guard;
mod refusal_cause;
mod request_facts;
mod rule_set;
mod rule_set_error;
mod runtime_error_kind;

pub use bundle_error::BundleError;
pub use bundle_handle::BundleHandle;
pub use bundle_loader::BundleLoader;
pub use cel_engine::CelEngine;
pub use cel_limits::{CelLimits, CelLimitsError};
pub use cel_refusal::CelRefusal;
pub use cel_verdict::CelVerdict;
pub use compile_error::CompileError;
pub use compiled_rule::CompiledRule;
pub use fact_name::{FactName, FactNameError};
pub use fact_value::FactValue;
pub use facts_limit_error::FactsLimitError;
pub use refusal_cause::RefusalCause;
pub use request_facts::RequestFacts;
pub use rule_set::RuleSet;
pub use rule_set_error::RuleSetError;
pub use runtime_error_kind::RuntimeErrorKind;
