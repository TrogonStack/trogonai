//! ACP elicitation provider.
//!
//! The transport primitives (`ElicitationReq`, the NATS forwarding handler, and
//! answer extraction) live in `trogon-runner-tools` and are shared by all
//! runners. This module only adds the `trogon-agent-core` `ElicitationProvider`
//! trait impl that the Claude agent loop expects, delegating to the shared
//! `elicit_via_channel`.

pub use trogon_runner_tools::elicitation::{ElicitationReq, ElicitationTx, handle_elicitation_request_nats};

use trogon_agent_core::agent_loop::ElicitationProvider;
use trogon_runner_tools::elicitation::elicit_via_channel;

/// Implements `ElicitationProvider` by routing requests through the shared
/// channel bridge to the ACP client.
pub struct ChannelElicitationProvider {
    pub session_id: String,
    pub tx: ElicitationTx,
}

impl ElicitationProvider for ChannelElicitationProvider {
    fn elicit<'a>(
        &'a self,
        question: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move { elicit_via_channel(&self.tx, &self.session_id, question).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// The provider delegates to the shared bridge and surfaces the answer.
    #[tokio::test]
    async fn provider_returns_answer() {
        use agent_client_protocol::schema::v1::{
            CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationContentValue,
        };
        use std::collections::BTreeMap;

        let (tx, mut rx) = mpsc::channel(8);
        let provider = ChannelElicitationProvider {
            session_id: "sess-1".to_string(),
            tx,
        };

        let task = tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let mut content = BTreeMap::new();
                content.insert("answer".to_string(), ElicitationContentValue::String("hi".to_string()));
                let resp = CreateElicitationResponse::new(ElicitationAction::Accept(
                    ElicitationAcceptAction::new().content(content),
                ));
                let _ = req.response_tx.send(Ok(resp));
            }
        });

        let result = provider.elicit("name?").await;
        task.await.unwrap();
        assert_eq!(result, Some("hi".to_string()));
    }
}
