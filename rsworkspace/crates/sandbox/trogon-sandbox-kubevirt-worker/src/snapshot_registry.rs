use async_trait::async_trait;
use futures::TryFutureExt;
use trogon_sandbox_contract::OciArtifactRef;
use trogon_sandbox_oci::{ArtifactType, Layer, OciClient, OciError, build_push_target};

/// The OCI registry snapshots are pulled from and pushed to.
#[async_trait]
pub trait SnapshotRegistry: Send + Sync {
    /// The layers of the artifact `reference` names, each verified against its digest.
    async fn pull(&self, reference: &OciArtifactRef) -> Result<Vec<Layer>, OciError>;

    /// Pushes `layer` as a `KubevirtDiskV1` artifact next to `target`'s repository and returns the reference it is
    /// stored under.
    async fn push(&self, target: &OciArtifactRef, layer: Layer) -> Result<OciArtifactRef, OciError>;
}

#[async_trait]
impl SnapshotRegistry for OciClient {
    async fn pull(&self, reference: &OciArtifactRef) -> Result<Vec<Layer>, OciError> {
        self.pull_artifact(reference).await.map(|pulled| pulled.layers)
    }

    #[rustfmt::skip]
    async fn push(&self, target: &OciArtifactRef, layer: Layer) -> Result<OciArtifactRef, OciError> {
        let layers = vec![layer];
        let stored = futures::future::ready(build_push_target(&target.registry, &target.repository, ArtifactType::KubevirtDiskV1, &layers, &Default::default()));
        stored.and_then(|stored| async move { self.push_artifact(&stored, layers, Default::default()).await.map(|_| stored) }).await
    }
}
