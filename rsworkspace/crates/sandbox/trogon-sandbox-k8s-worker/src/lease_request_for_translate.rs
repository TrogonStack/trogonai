use trogon_sandbox_contract::{LeaseRequest, SandboxSpec, WorkspaceSource};

use crate::K8sWorkerError;

/// `req` with an S3 volume workspace replaced by an empty one, which the
/// translation to a provider spec refuses; the worker syncs the volume itself.
#[expect(
    clippy::result_large_err,
    reason = "K8sWorkerError carries the provider errors by value so callers can match on them"
)]
pub(crate) fn lease_request_for_translate(req: &LeaseRequest) -> Result<LeaseRequest, K8sWorkerError> {
    let mut out = req.clone();
    if matches!(out.spec.workspace(), WorkspaceSource::S3Volume { .. }) {
        let mut parts = out.spec.into_parts();
        parts.workspace = WorkspaceSource::empty();
        out.spec = SandboxSpec::new(parts)?;
    }
    Ok(out)
}
