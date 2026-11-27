//! A throwaway NATS server for tests that need a real connection.

#![allow(dead_code)]

use async_nats::Client;
use trogon_nats::test_support::CoreTestServer;

/// A NATS server running in a container, stopped when dropped.
pub struct Server {
    inner: CoreTestServer,
}

impl Server {
    /// Starts a server with default settings.
    pub async fn start() -> Self {
        Self {
            inner: CoreTestServer::start().await,
        }
    }

    /// Starts a server configured by the given server configuration text.
    pub async fn start_with_config(config: &str) -> Self {
        Self {
            inner: CoreTestServer::start_with_config(config).await,
        }
    }

    /// The URL clients connect to.
    pub fn url(&self) -> String {
        format!("nats://{}", self.inner.address())
    }

    /// Connects an anonymous client.
    pub async fn client(&self) -> Result<Client, async_nats::ConnectError> {
        async_nats::connect(self.url()).await
    }
}

/// Starts a server and connects a client to it.
pub async fn start_nats() -> Result<(Server, Client), async_nats::ConnectError> {
    let server = Server::start().await;
    let client = server.client().await?;
    Ok((server, client))
}
