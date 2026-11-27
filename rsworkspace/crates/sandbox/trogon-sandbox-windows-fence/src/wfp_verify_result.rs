use crate::wfp_residual::WfpResidual;
use crate::wfp_verify_probes::WfpVerifyProbes;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WfpVerifyResult {
    pub target: String,
    pub stderr: String,
    pub probes: WfpVerifyProbes,
    /// Gaps the probe found open: not a clean pass when non-empty.
    pub residuals: Vec<WfpResidual>,
    pub warnings: Vec<String>,
}
