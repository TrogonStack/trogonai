use trogon_sandbox_contract::{Digest, WasmCapabilityGrant};

use crate::{Limits, ReviewPolicy, ToolId};

/// A digest-pinned component and only that component's admitted capabilities.
#[derive(Debug, Clone)]
pub struct WasmTool {
    pub(crate) id: ToolId,
    pub(crate) component: Digest,
    pub(crate) capabilities: WasmCapabilityGrant,
    pub(crate) review: ReviewPolicy,
    pub(crate) limits: Limits,
}

impl WasmTool {
    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn id(&self) -> &ToolId {
        &self.id
    }
    pub fn component(&self) -> &Digest {
        &self.component
    }
    pub fn capabilities(&self) -> &WasmCapabilityGrant {
        &self.capabilities
    }
}
