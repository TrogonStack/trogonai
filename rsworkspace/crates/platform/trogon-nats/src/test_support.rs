//! Shared NATS infrastructure for integration tests.

use std::time::Duration;

use async_nats::jetstream;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};

const NATS_IMAGE_TAG: &str = "2.10.14";
const NATS_CLIENT_PORT: u16 = 4222;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// An isolated NATS server with JetStream enabled.
pub struct JetStreamTestServer {
    _container: ContainerAsync<Nats>,
    address: String,
}

impl JetStreamTestServer {
    /// Starts the pinned NATS image and waits until it accepts client connections.
    pub async fn start() -> Self {
        let command = NatsServerCmd::default().with_jetstream();
        let container = Nats::default()
            .with_tag(NATS_IMAGE_TAG)
            .with_cmd(&command)
            .start()
            .await
            .expect("start JetStream testcontainer");
        let host = container.get_host().await.expect("get NATS testcontainer host");
        let port = container
            .get_host_port_ipv4(NATS_CLIENT_PORT)
            .await
            .expect("get NATS testcontainer port");

        Self {
            _container: container,
            address: format!("{host}:{port}"),
        }
    }

    /// Connects to the isolated server and returns its JetStream context.
    pub async fn jetstream(&self) -> jetstream::Context {
        let client = async_nats::ConnectOptions::new()
            .connection_timeout(CONNECT_TIMEOUT)
            .connect(&self.address)
            .await
            .expect("connect to JetStream testcontainer");
        jetstream::new(client)
    }
}
