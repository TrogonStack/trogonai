//! Shared NATS infrastructure for integration tests.

use std::time::Duration;

use async_nats::jetstream;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_std::env::SystemEnv;

use crate::auth::NatsConfig;
use crate::connect::build_options;
use crate::tls::NatsTls;

const NATS_IMAGE_TAG: &str = "2.10.14";
const NATS_CLIENT_PORT: u16 = 4222;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The pinned NATS image, started and reachable, without saying which server
/// features the test needs. Both public servers wrap one of these so the image
/// tag, the published port, and the connect timeout are decided once.
struct TestServer {
    _container: ContainerAsync<Nats>,
    address: String,
}

impl TestServer {
    async fn start(command: &NatsServerCmd) -> Self {
        let container = Nats::default()
            .with_tag(NATS_IMAGE_TAG)
            .with_cmd(command)
            .start()
            .await
            .expect("start NATS testcontainer");
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

    async fn client(&self) -> async_nats::Client {
        let config = NatsConfig::from_url(&self.address).with_tls(NatsTls::Disabled);
        build_options(&config, &SystemEnv)
            .await
            .expect("build NATS testcontainer client options")
            .connection_timeout(CONNECT_TIMEOUT)
            .connect(&self.address)
            .await
            .expect("connect to NATS testcontainer")
    }
}

/// An isolated NATS server with JetStream enabled.
pub struct JetStreamTestServer(TestServer);

impl JetStreamTestServer {
    /// The mapped host and port for clients that manage their own connections.
    /// The server does not speak TLS, so a configuration that connects here
    /// has to disable it.
    pub fn address(&self) -> &str {
        &self.0.address
    }

    /// Starts the pinned NATS image and waits until it accepts client connections.
    pub async fn start() -> Self {
        Self(TestServer::start(&NatsServerCmd::default().with_jetstream()).await)
    }

    /// Connects to the isolated server and returns its JetStream context.
    pub async fn jetstream(&self) -> jetstream::Context {
        jetstream::new(self.client().await)
    }

    /// A raw connection to the isolated server, for tests that need a context
    /// built some other way (a non-default API prefix, a domain).
    pub async fn client(&self) -> async_nats::Client {
        self.0.client().await
    }
}

/// An isolated NATS server with only core NATS, for bindings that live on
/// request/reply and NATS Services rather than on streams.
///
/// JetStream is left off deliberately: a test that never opens a stream should
/// not be able to pass by accidentally depending on one.
pub struct CoreTestServer(TestServer);

impl CoreTestServer {
    /// Starts the pinned NATS image and waits until it accepts client connections.
    pub async fn start() -> Self {
        Self(TestServer::start(&NatsServerCmd::default()).await)
    }

    /// The `host:port` this server is reachable on, for tests that connect
    /// through their own configuration rather than a raw client. The server
    /// does not speak TLS, so that configuration has to disable it.
    pub fn address(&self) -> &str {
        &self.0.address
    }
}
