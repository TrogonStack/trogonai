/// A list of idle stages the evaluator could not run unambiguously.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum IdlePolicyError {
    #[error("idle_policy needs at least one stage")]
    NoStages,
    #[error("idle_policy stages must be sorted by strictly increasing after_seconds")]
    UnorderedStages,
    #[error("idle_policy actions must get strictly more severe (pause, suspend, then release)")]
    NonEscalatingActions,
}
