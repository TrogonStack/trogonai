use std::sync::Arc;

/// Decides, before the agent sees a request, whether the client that sent it
/// is served.
///
/// `client` is the name in the request's
/// [`ACP_CLIENT_HEADER`](acp_nats::ACP_CLIENT_HEADER), which the `acp-nats`
/// bridge sets from its client's `initialize`; `None` when the request carries
/// none (a client that sent no `clientInfo`, or an older bridge). A refusal is
/// sent back as the request's JSON-RPC error; a refused notification is dropped.
pub trait ClientGate {
    fn admit(&self, client: Option<&str>) -> Result<(), agent_client_protocol::Error>;
}

/// Serves every client: what an agent that may be shared uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdmitAll;

impl ClientGate for AdmitAll {
    fn admit(&self, _client: Option<&str>) -> Result<(), agent_client_protocol::Error> {
        Ok(())
    }
}

impl<G: ClientGate + ?Sized> ClientGate for Arc<G> {
    fn admit(&self, client: Option<&str>) -> Result<(), agent_client_protocol::Error> {
        (**self).admit(client)
    }
}
