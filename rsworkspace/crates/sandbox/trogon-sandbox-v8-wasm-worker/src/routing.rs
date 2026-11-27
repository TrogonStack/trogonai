use std::sync::Arc;

use trogon_sandbox_contract::WorkerId;

use crate::V8WasmProvider;

/// What the dispatch handlers need to stamp a grant with its worker and to track routed leases.
pub(crate) struct Routing {
    pub(crate) host: Arc<V8WasmProvider>,
    pub(crate) worker: WorkerId,
    /// Whether the dispatched provider is not the host itself, so the host must learn of its leases.
    pub(crate) external: bool,
}
