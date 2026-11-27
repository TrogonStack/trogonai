use trogon_sandbox_contract::{LeaseRequest, OciArtifactRef, SandboxImage, SandboxSpec};

use crate::K8sWorkerError;

/// The request to relaunch with: `request` with `snapshot` as its image. A
/// registry image becomes the snapshot's parent, so the rebuilt sandbox runs
/// the image the lease started on rather than the worker's default.
#[expect(
    clippy::result_large_err,
    reason = "K8sWorkerError carries the provider errors by value so callers can match on them"
)]
pub(crate) fn request_with_snapshot(
    request: &LeaseRequest,
    snapshot: &OciArtifactRef,
) -> Result<LeaseRequest, K8sWorkerError> {
    let parent = match request.spec.image() {
        SandboxImage::Registry { reference, .. } => Some(reference.clone()),
        SandboxImage::Snapshot { parent, .. } => parent.clone(),
        _ => None,
    };
    let mut out = request.clone();
    let mut parts = out.spec.into_parts();
    parts.image = SandboxImage::Snapshot {
        reference: snapshot.clone(),
        parent,
    };
    out.spec = SandboxSpec::new(parts)?;
    Ok(out)
}

#[cfg(test)]
mod tests;
