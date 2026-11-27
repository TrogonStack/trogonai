use rmcp::model::CallToolResult;
use serde_json::Value;

use crate::policy::Policy;
use crate::testing::{McpSession, Reply, ScriptedBus, Sent, call_result_text, test_server};

/// What one tool call did: the result the client saw and every request the server put on the bus.
pub(super) struct Outcome {
    pub(super) result: CallToolResult,
    pub(super) sent: Vec<Sent>,
}

impl Outcome {
    pub(super) fn is_error(&self) -> bool {
        self.result.is_error == Some(true)
    }

    pub(super) fn text(&self) -> Result<String, serde_json::Error> {
        call_result_text(&self.result)
    }
}

/// Calls `tool` on a server whose bus answers `replies` in order.
pub(super) async fn call_with(
    policy: Policy,
    tool: &'static str,
    args: Value,
    replies: Vec<Reply>,
) -> Result<Outcome, Box<dyn std::error::Error>> {
    let bus = ScriptedBus::scripted(replies);
    let log = bus.log();
    let session = McpSession::start(test_server(bus, policy)?).await?;
    let result = session.call(tool, args).await?;
    session.finish().await?;
    Ok(Outcome {
        result,
        sent: log.all(),
    })
}

/// Calls `tool` with the default policy and one reply.
pub(super) async fn call(
    tool: &'static str,
    args: Value,
    reply: &impl serde::Serialize,
) -> Result<Outcome, Box<dyn std::error::Error>> {
    call_with(Policy::default(), tool, args, vec![Reply::json(reply)?]).await
}
