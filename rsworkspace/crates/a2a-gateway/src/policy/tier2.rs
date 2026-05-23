use crate::policy::error::Tier2EvalError;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CelProgramRef<'a>(pub &'a str);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PolicyEnvelopeBlob<'a>(pub &'a [u8]);

pub trait Tier2CelEvaluator: Send + Sync {
    fn predicate_holds(
        &self,
        program: CelProgramRef<'_>,
        envelope: PolicyEnvelopeBlob<'_>,
    ) -> Result<bool, Tier2EvalError>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct NoopTier2Evaluator;

impl Tier2CelEvaluator for NoopTier2Evaluator {
    fn predicate_holds(
        &self,
        _program: CelProgramRef<'_>,
        _envelope: PolicyEnvelopeBlob<'_>,
    ) -> Result<bool, Tier2EvalError> {
        Ok(true)
    }
}
