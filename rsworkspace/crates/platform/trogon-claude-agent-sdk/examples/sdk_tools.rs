#[path = "support/schema.rs"]
mod schema;

use std::sync::Arc;

use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, LocalMcpServer, LocalMcpServerOptions, McpServerName, McpToolInputSchema, McpToolName,
    McpToolOutput, McpToolRequest, Message, Prompt, SdkMcpServers, SdkTool, SdkToolOptions, ToolRule, query,
};

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        debug_remnants,
        reason = "This example prints its defined result or connection status to the caller."
    )
)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = McpToolInputSchema::new(schema::echo_schema(Some(false))?);
    let echo = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("echo"),
        description: "Return the supplied text".into(),
        input_schema: schema,
        handler: Arc::new(|request: McpToolRequest| async move {
            Ok(McpToolOutput::text(
                request.arguments["text"].as_str().unwrap_or_default(),
            ))
        }),
    });
    let server_name = McpServerName::from("example");
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        name: server_name.clone(),
        version: "1.0.0".into(),
        tools: echo.into(),
    });
    let options = ClaudeAgentOptions {
        sdk_mcp_servers: SdkMcpServers::from([(
            server_name,
            Arc::new(server) as Arc<dyn trogon_claude_agent_sdk::SdkMcpServer>,
        )]),
        allowed_tools: vec![ToolRule::from("mcp__example__echo")],
        ..ClaudeAgentOptions::default()
    };
    let mut response = query(Prompt::from("Use the example echo tool to echo hello."), options).await?;
    while let Some(message) = response.next_message().await? {
        if let Message::Result(result) = message {
            println!("{}", result.result.unwrap_or_default());
        }
    }
    Ok(())
}
