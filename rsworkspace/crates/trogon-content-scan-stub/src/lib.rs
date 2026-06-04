//! Default `contentscan` plugin: always allows, emits one tracing
//! audit record per request. Integrators replace this binary with a
//! real scanner (Lakera, Model Armor, in-house) but keep the
//! [`ContentScanRequest`]/[`ContentScanDecision`] wire contract.
//!
//! Use cases:
//! - Tests that need a conforming subscriber without standing up a
//!   real scanner.
//! - Bootstrap / development environments where DLP isn't wired up
//!   yet but the gateway is configured to require a scanner reply.

use std::sync::Arc;

use async_nats::Client;
use futures::StreamExt;
use tracing::{info, warn};
use trogon_mcp_gateway::plugin::{
    CONTENT_SCAN_PLUGIN_NAME, ContentScanDecision, ContentScanRequest, ContentScanner, plugin_subject,
};

/// Always-allow scanner. Logs one structured `content_scan` audit
/// record per call so operators can see traffic flowing.
#[derive(Debug, Default, Clone)]
pub struct StubContentScanner;

#[async_trait::async_trait]
impl ContentScanner for StubContentScanner {
    async fn scan(&self, req: &ContentScanRequest) -> ContentScanDecision {
        info!(
            target: "content_scan",
            request_id = %req.request_id,
            tenant = %req.tenant,
            agent = %req.agent,
            tool = %req.tool,
            stage = req.stage.as_str(),
            direction = ?req.direction,
            content_type = %req.content_type,
            content_len = req.content.len(),
            "stub scanner allow"
        );
        ContentScanDecision::Allow
    }
}

/// Subscribe on `{prefix}.plugin.contentscan` (queue group
/// `mcp-plugin-contentscan`) and reply with the scanner's verdict.
/// Returns when the subscription ends or the runtime is shut down.
pub async fn serve(
    client: Client,
    prefix: &str,
    scanner: Arc<dyn ContentScanner>,
) -> Result<(), async_nats::Error> {
    let subject = plugin_subject(prefix, CONTENT_SCAN_PLUGIN_NAME);
    let queue = format!("mcp-plugin-{CONTENT_SCAN_PLUGIN_NAME}");
    let mut sub = client.queue_subscribe(subject.clone(), queue).await?;
    info!(%subject, "content scan stub subscribed");
    while let Some(msg) = sub.next().await {
        let Some(reply) = msg.reply.clone() else {
            warn!("content scan request without reply subject; dropping");
            continue;
        };
        let decision = match serde_json::from_slice::<ContentScanRequest>(&msg.payload) {
            Ok(req) => scanner.scan(&req).await,
            Err(e) => {
                warn!(error = %e, "malformed content scan request");
                ContentScanDecision::Block {
                    reason: Some(format!("malformed_request: {e}")),
                    findings: vec![],
                }
            }
        };
        let body = match serde_json::to_vec(&decision) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "failed to encode content scan decision");
                continue;
            }
        };
        if let Err(e) = client.publish(reply, body.into()).await {
            warn!(error = %e, "failed to publish content scan reply");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_mcp_gateway::plugin::{
        ContentScanDirection, ContentScanRequest, PluginStage, plugin_subject,
    };

    fn req(stage: PluginStage, direction: ContentScanDirection) -> ContentScanRequest {
        ContentScanRequest {
            request_id: "r-1".into(),
            tenant: "acme".into(),
            agent: "search-bot".into(),
            tool: "tools/call".into(),
            stage,
            direction,
            content_type: "application/json".into(),
            content: "hello".into(),
        }
    }

    #[tokio::test]
    async fn stub_allows_inbound_and_outbound() {
        let s = StubContentScanner;
        let pre = s.scan(&req(PluginStage::PreCall, ContentScanDirection::Inbound)).await;
        let post = s.scan(&req(PluginStage::PostCall, ContentScanDirection::Outbound)).await;
        assert!(pre.is_allow());
        assert!(post.is_allow());
    }

    #[test]
    fn subject_is_well_known() {
        assert_eq!(plugin_subject("mcp", CONTENT_SCAN_PLUGIN_NAME), "mcp.plugin.contentscan");
    }
}
