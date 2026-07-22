#![cfg_attr(coverage, feature(coverage_attribute))]
//! MCP (Model Context Protocol) clients for trogon.
//!
//! Two transports are provided:
//! - [`McpClient`] — streamable-HTTP (JSON-RPC over POST).
//! - [`StdioMcpClient`] — native in-process stdio: drives a server subprocess
//!   over its stdin/stdout, so stdio servers need no local HTTP bridge process.
//!
//! Both discover tools and dispatch tool calls via the shared [`McpCallTool`].
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
mod stdio;

pub use client::{McpCallTool, McpClient, McpPrompt, McpPromptArgument, McpResource, McpResourceContent, McpTool};
pub use stdio::StdioMcpClient;

#[cfg(feature = "test-support")]
pub use client::mock::MockMcpClient;
