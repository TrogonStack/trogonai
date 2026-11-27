use std::collections::BTreeMap;

use async_trait::async_trait;
use kube::core::DynamicObject;
use trogon_sandbox_contract::OciArtifactRef;
use trogon_sandbox_volumes::VolumeClientConfig;

use crate::{KubeVirtOps, WarmDiskSourceError};

/// Builds the root disk of a warm VM: a DataVolume named `dv_name`, carrying `labels`, that restores the
/// snapshot `reference` names through the `volume` store.
///
/// The source only describes the DataVolume. [`crate::WarmPoolManager`] creates it with a receipt and waits
/// for the import, so a failure anywhere after this call is cleaned up by uid.
#[async_trait]
pub trait WarmDiskSource: Send + Sync {
    async fn data_volume(
        &self,
        ops: &KubeVirtOps,
        volume: &VolumeClientConfig,
        reference: &OciArtifactRef,
        dv_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<DynamicObject, WarmDiskSourceError>;
}
