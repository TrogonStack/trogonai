/// Why the Trivy scan gate did not pass.
#[derive(Debug, thiserror::Error)]
pub enum TrivyError {
    #[error("trivy binary not found")]
    NotInstalled,
    #[error("trivy scan failed: {0}")]
    ScanFailed(String),
    #[error("trivy policy violation: max CVSS {max_score:.1} >= threshold {threshold}")]
    PolicyViolation { max_score: f64, threshold: f64 },
    #[error("parse trivy report: {0}")]
    Parse(#[source] serde_json::Error),
}
