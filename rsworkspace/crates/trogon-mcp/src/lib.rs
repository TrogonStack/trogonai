//! MCP (Model Context Protocol) HTTP client for trogon.
//!
//! Connects to MCP servers via the streamable-HTTP transport (JSON-RPC over
//! POST), discovers their tools, and dispatches tool calls.
//!
//! # Usage
//!
//! ```no_run
//! # async fn example() -> Result<(), String> {
//! let client = trogon_mcp::McpClient::new(reqwest::Client::new(), "http://mcp-server/mcp");
//! client.initialize().await?;
//! let tools = client.list_tools().await?;
//! let output = client.call_tool("my_tool", &serde_json::json!({"key": "val"})).await?;
//! # Ok(()) }
//! ```

mod client;

pub use client::{McpClient, McpTool};
