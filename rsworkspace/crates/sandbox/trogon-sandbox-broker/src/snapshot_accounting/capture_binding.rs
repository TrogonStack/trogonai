use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::OciArtifactRef;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CaptureBinding {
    pub reference: OciArtifactRef,
    pub bytes: u64,
}
