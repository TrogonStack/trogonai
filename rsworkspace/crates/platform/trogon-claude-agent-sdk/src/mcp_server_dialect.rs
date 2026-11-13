/// Which upstream SDK's in-process MCP server a `LocalMcpServer` answers like.
///
/// The TypeScript SDK serves SDK tools through the `@modelcontextprotocol/sdk` `McpServer`, and the Python
/// SDK through the `mcp` package's lowlevel `Server`. The two differ in envelopes, error codes, request
/// gating, argument validation messages and result shapes, so the dialect is chosen per server.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum McpServerDialect {
    /// `createSdkMcpServer` from the TypeScript SDK.
    #[default]
    TypeScript,
    /// `create_sdk_mcp_server` from the Python SDK.
    Python,
}
