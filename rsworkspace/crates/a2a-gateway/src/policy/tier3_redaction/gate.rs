use super::context::Tier3EvaluationContext;
use super::decision::Tier3RedactionDecision;

pub trait Tier3RedactionGate: Send + Sync {
    fn redact(&self, ctx: &mut Tier3EvaluationContext) -> Tier3RedactionDecision;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct NoopTier3RedactionGate;

impl Tier3RedactionGate for NoopTier3RedactionGate {
    fn redact(&self, _ctx: &mut Tier3EvaluationContext) -> Tier3RedactionDecision {
        Tier3RedactionDecision::Allow {
            rewrites: Vec::new(),
        }
    }
}
