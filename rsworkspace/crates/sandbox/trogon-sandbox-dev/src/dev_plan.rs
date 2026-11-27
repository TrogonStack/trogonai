use crate::{ProcessSpec, WorkerKind};

/// The processes of a local stack and the URLs clients reach it on.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DevPlan {
    pub nats_url: String,
    pub gateway_http_url: String,
    pub gateway_ws_url: String,
    pub nats: ProcessSpec,
    /// Broker, worker and gateway, started after NATS accepts connections.
    pub services: Vec<ProcessSpec>,
    pub worker: WorkerKind,
}
