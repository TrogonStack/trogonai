/// Message direction disambiguates an absent `Jsonrpc-Id` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Inbound or outbound JSON-RPC request (or notification).
    Request,
    /// Inbound or outbound JSON-RPC response.
    Response,
}
