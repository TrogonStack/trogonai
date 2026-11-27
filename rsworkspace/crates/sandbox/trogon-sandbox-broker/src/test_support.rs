use trogon_nats::test_support::JetStreamTestServer;

/// A throwaway JetStream-enabled NATS server running in a container, with a connected client.
pub(crate) struct LocalNats {
    _server: JetStreamTestServer,
    pub(crate) client: async_nats::Client,
}

impl LocalNats {
    pub(crate) async fn start() -> Self {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        Self {
            _server: server,
            client,
        }
    }

    /// A new connection to the same server, for tests that rebuild their client.
    pub(crate) async fn fresh_client(&self) -> async_nats::Client {
        self._server.client().await
    }
}
