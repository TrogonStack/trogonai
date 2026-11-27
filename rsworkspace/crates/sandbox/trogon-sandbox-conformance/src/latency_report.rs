use crate::LatencyStep;

/// The latency of one worker, step by step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyReport {
    pub provider: String,
    pub steps: Vec<LatencyStep>,
}
