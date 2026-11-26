use serde::Serialize;

use crate::WsGatewayUrl;

/// Runtime configuration exposed to the single-page app through `/config.json`.
#[derive(Clone, Debug, Serialize)]
pub struct DashboardConfig {
    pub ws_gateway_url: WsGatewayUrl,
}
