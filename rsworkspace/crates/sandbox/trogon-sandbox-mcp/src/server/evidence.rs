//! Evidence frames reach MCP clients as logging notifications, the only server-to-client stream rmcp offers.
//! Logging is deprecated upstream with no replacement yet, so the deprecated calls live in these two helpers and
//! nowhere else.

#[allow(deprecated)]
use rmcp::model::{LoggingLevel, LoggingMessageNotificationParam, ServerCapabilities};
use rmcp::{Peer, RoleServer, ServiceError};

use crate::constants::EVIDENCE_LOGGER;

#[allow(deprecated)]
pub(super) fn advertised_capabilities() -> ServerCapabilities {
    ServerCapabilities::builder().enable_tools().enable_logging().build()
}

#[allow(deprecated)]
pub(super) async fn notify_evidence(peer: &Peer<RoleServer>, data: serde_json::Value) -> Result<(), ServiceError> {
    let params = LoggingMessageNotificationParam::new(LoggingLevel::Info, data).with_logger(EVIDENCE_LOGGER);
    peer.notify_logging_message(params).await
}
