/// A readiness needle does not satisfy the rules [`ReadinessNeedle`](crate::ReadinessNeedle) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ReadinessNeedleError {
    #[error("readiness needle must be 1 to {max} bytes")]
    Length { max: usize },
}
