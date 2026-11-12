use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Customizations, McpDiscovery, SessionPersistence, ToolSet,
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
    let options = ClaudeAgentOptions {
        tools: ToolSet::Only(Vec::new()),
        mcp_discovery: McpDiscovery::Disabled,
        session_persistence: SessionPersistence::Disabled,
        customizations: Customizations::Bare,
        ..ClaudeAgentOptions::default()
    };
    let client = ClaudeSdkClient::connect(options).await?;
    client.disconnect().await?;
    println!("Claude Code initialized and closed without a prompt.");
    Ok(())
}
