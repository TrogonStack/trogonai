// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use trogon_nats::test_support::JetStreamTestServer;

/// A throwaway JetStream-enabled NATS server and a client connected to it.
pub struct LocalNats {
    _server: JetStreamTestServer,
    pub client: async_nats::Client,
}

impl LocalNats {
    pub async fn start() -> Self {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        Self {
            _server: server,
            client,
        }
    }
}
