use trogon_sandbox_contract::{CapabilitySupport, WorkerDispatchProtocol, WorkerId};

#[derive(Clone)]
pub(crate) struct SelectedWorker {
    pub(crate) id: WorkerId,
    pub(crate) protocol: WorkerDispatchProtocol,
    pub(crate) supports_lease_epoch: CapabilitySupport,
}
