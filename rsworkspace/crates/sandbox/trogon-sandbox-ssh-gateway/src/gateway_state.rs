use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::GatewayConfig;

/// The configuration of a running gateway and the connection slots it hands out.
pub struct GatewayState {
    pub(crate) config: GatewayConfig,
    pub(crate) connection_slots: Arc<Semaphore>,
}

impl GatewayState {
    pub fn new(config: GatewayConfig) -> Self {
        let connection_slots = Arc::new(Semaphore::new(config.max_connections.get()));
        Self {
            config,
            connection_slots,
        }
    }
}
