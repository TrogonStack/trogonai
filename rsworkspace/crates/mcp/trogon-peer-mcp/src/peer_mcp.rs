use rmcp::ServerHandler;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData};
use rmcp::service::{RequestContext, RoleServer};

use crate::{ConnectorConfigInput, EnrollPeer, PeerMcpError, PreparePeerBatch};

pub struct PeerMcp<B> {
    backend: B,
    config: ConnectorConfigInput,
}

impl<B> PeerMcp<B> {
    pub fn new(backend: B, config: ConnectorConfigInput) -> Result<Self, PeerMcpError> {
        Ok(Self { backend, config })
    }

    pub async fn invoke(&self, _request: CallToolRequestParams) -> Result<CallToolResult, PeerMcpError> {
        let _ = (&self.backend, &self.config);
        Err(PeerMcpError::Unbound)
    }
}

impl<B: EnrollPeer + PreparePeerBatch> ServerHandler for PeerMcp<B> {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.invoke(request)
            .await
            .map(CallToolResponse::Complete)
            .map_err(|_| ErrorData::internal_error("peer adapter is unavailable", None))
    }
}
