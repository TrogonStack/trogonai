use std::sync::Arc;

use crate::jsonrpc_connection::JsonRpcConnection;
use crate::lsp_transport::LspTransport;

#[derive(Clone)]
pub(crate) struct Runtime {
    pub(crate) transport: Arc<dyn LspTransport>,
    pub(crate) connection: Arc<JsonRpcConnection>,
}
