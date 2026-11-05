use crate::narrowing::Narrowing;

/// An engine that evaluates request rules: a declarative rule set, a CEL
/// predicate set, a Wasm module, or a guardrail.
///
/// This crate defines the trait and the CEL crate implements it, so evaluation
/// stays free of any rule language. The result type admits no widening: an
/// engine can refuse, and it cannot permit.
///
/// An engine that fails, times out, or cannot make sense of its inputs must
/// answer [`Narrowing::Deny`]. A request rule that errors has not passed.
pub trait RequestRuleEngine {
    /// What the engine reads about a request. Neutral, owned by the engine.
    type Facts: ?Sized;

    /// Decides whether any rule refuses these facts.
    fn narrow(&self, facts: &Self::Facts) -> Narrowing;
}
