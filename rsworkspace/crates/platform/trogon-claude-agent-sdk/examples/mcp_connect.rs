#[path = "support/schema.rs"]
mod schema;

use std::{sync::Arc, time::Duration};

use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Customizations, LocalMcpServer, LocalMcpServerOptions, McpServerDialect,
    McpServerName, McpToolInputSchema, McpToolName, McpToolOutput, McpToolRequest, SdkMcpServer, SdkMcpServers,
    SdkTool, SdkToolOptions, SessionPersistence, ToolSet,
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
    let directory = tempfile::tempdir()?;
    let name = McpServerName::from("example");
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("echo"),
        description: "Return the supplied text".into(),
        input_schema: McpToolInputSchema::new(schema::echo_schema(None)?),
        handler: Arc::new(|request: McpToolRequest| async move {
            Ok(McpToolOutput::text(
                request.arguments["text"].as_str().unwrap_or_default(),
            ))
        }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        name: name.clone(),
        version: Some("1.0.0".into()),
        tools: Some(tool.into()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    });
    let options = ClaudeAgentOptions {
        cwd: Some(directory.path().to_path_buf()),
        tools: ToolSet::Only(Vec::new()),
        customizations: Customizations::Bare,
        session_persistence: SessionPersistence::Disabled,
        sdk_mcp_servers: SdkMcpServers::from([(name, Arc::new(server) as Arc<dyn SdkMcpServer>)]),
        ..ClaudeAgentOptions::default()
    };
    let client = ClaudeSdkClient::connect(options).await?;
    let handle = client.handle();
    let connected = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let status = handle.mcp_status().await?;
            if status.mcp_servers.iter().any(|server| {
                server.get("name").and_then(serde_json::Value::as_str) == Some("example")
                    && server.get("status").and_then(serde_json::Value::as_str) == Some("connected")
            }) {
                return Ok::<(), trogon_claude_agent_sdk::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    client.disconnect().await?;
    connected??;
    println!("SDK MCP connected without sending a prompt.");
    Ok(())
}
