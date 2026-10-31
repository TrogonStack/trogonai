//! Shared NATS infrastructure for integration tests.

use std::path::PathBuf;
use std::time::Duration;

use async_nats::jetstream;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
};
use tempfile::TempDir;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ContainerRequest, ImageExt, runners::AsyncRunner};
use trogon_std::env::SystemEnv;

use crate::auth::NatsConfig;
use crate::connect::build_options;
use crate::tls::NatsTls;

const NATS_IMAGE_TAG: &str = "2.10.14";
const NATS_CLIENT_PORT: u16 = 4222;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SERVER_NAMES: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
const SERVER_CERT: &str = "/tls/server-cert.pem";
const SERVER_KEY: &str = "/tls/server-key.pem";
const SERVER_CLIENT_CA: &str = "/tls/client-ca.pem";
const CA_FILE: &str = "ca.pem";
const CLIENT_CERT_FILE: &str = "client-cert.pem";
const CLIENT_KEY_FILE: &str = "client-key.pem";

fn image(command: impl IntoIterator<Item = impl Into<String>>) -> ContainerRequest<Nats> {
    Nats::default().with_tag(NATS_IMAGE_TAG).with_cmd(command)
}

/// The pinned NATS image, started and reachable, without saying which server
/// features the test needs. Every public server wraps one of these so the
/// image tag, the published port, and the connect timeout are decided once.
struct TestServer {
    _container: ContainerAsync<Nats>,
    address: String,
}

impl TestServer {
    async fn start(request: ContainerRequest<Nats>) -> Self {
        let container = request.start().await.expect("start NATS testcontainer");
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
        Self(TestServer::start(image(&NatsServerCmd::default().with_jetstream())).await)
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
        Self(TestServer::start(image(&NatsServerCmd::default())).await)
    }

    /// The `host:port` this server is reachable on, for tests that connect
    /// through their own configuration rather than a raw client. The server
    /// does not speak TLS, so that configuration has to disable it.
    pub fn address(&self) -> &str {
        &self.0.address
    }
}

/// A certificate and its private key, PEM encoded.
struct IssuedCertificate {
    cert: String,
    key: String,
}

impl IssuedCertificate {
    fn signed_by(mut params: CertificateParams, common_name: &str, ca: &Issuer<'_, KeyPair>) -> Self {
        params.distinguished_name.push(DnType::CommonName, common_name);
        let key = KeyPair::generate().expect("generate a test key");
        let cert = params.signed_by(&key, ca).expect("issue a test certificate");

        Self {
            cert: cert.pem(),
            key: key.serialize_pem(),
        }
    }
}

/// A CA generated for one server, with the certificates it issued to that
/// server and to a client, so no key outlives the test that made it.
struct TestCertificates {
    ca: String,
    server: IssuedCertificate,
    client: IssuedCertificate,
}

impl TestCertificates {
    fn generate() -> Self {
        let mut ca = CertificateParams::default();
        ca.distinguished_name.push(DnType::CommonName, "trogon-nats test CA");
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate().expect("generate the test CA key"))
            .expect("self-sign the test CA");

        let mut server = CertificateParams::new(SERVER_NAMES.map(String::from)).expect("name the test server");
        server.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let mut client = CertificateParams::default();
        client.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

        Self {
            ca: ca.pem(),
            server: IssuedCertificate::signed_by(server, "trogon-nats test server", &ca),
            client: IssuedCertificate::signed_by(client, "trogon-nats test client", &ca),
        }
    }
}

/// Whether a [`TlsTestServer`] asks clients for a certificate.
#[derive(Clone, Copy)]
enum ClientCertificates {
    NotRequested,
    Required,
}

impl ClientCertificates {
    fn server_flags(self) -> &'static [&'static str] {
        match self {
            Self::NotRequested => &["--tls"],
            Self::Required => &["--tlsverify", "--tlscacert", SERVER_CLIENT_CA],
        }
    }
}

/// An isolated core NATS server that only accepts TLS, with a CA generated
/// for it alone.
///
/// The CA and a client certificate it issued are written to a temporary
/// directory that lives as long as the server, so a test points `NATS_CA`,
/// `NATS_CERT`, and `NATS_KEY` at files the way a deployment does.
pub struct TlsTestServer {
    server: TestServer,
    files: TempDir,
}

impl TlsTestServer {
    /// Starts a server that proves its identity to clients but asks none of
    /// them for a certificate.
    pub async fn start() -> Self {
        Self::start_with(ClientCertificates::NotRequested).await
    }

    /// Starts a server that turns away every client without a certificate
    /// issued by its CA.
    pub async fn start_verifying_clients() -> Self {
        Self::start_with(ClientCertificates::Required).await
    }

    async fn start_with(clients: ClientCertificates) -> Self {
        let certificates = TestCertificates::generate();
        let files = tempfile::tempdir().expect("create a directory for TLS test files");
        for (name, pem) in [
            (CA_FILE, &certificates.ca),
            (CLIENT_CERT_FILE, &certificates.client.cert),
            (CLIENT_KEY_FILE, &certificates.client.key),
        ] {
            std::fs::write(files.path().join(name), pem).expect("write a TLS test file");
        }

        let mut command = vec!["--tlscert", SERVER_CERT, "--tlskey", SERVER_KEY];
        command.extend(clients.server_flags());
        let request = image(command)
            .with_copy_to(SERVER_CERT, certificates.server.cert.into_bytes())
            .with_copy_to(SERVER_KEY, certificates.server.key.into_bytes())
            .with_copy_to(SERVER_CLIENT_CA, certificates.ca.into_bytes());

        Self {
            server: TestServer::start(request).await,
            files,
        }
    }

    /// The `host:port` this server is reachable on. Its certificate names
    /// `localhost` and the loopback addresses.
    pub fn address(&self) -> &str {
        &self.server.address
    }

    /// The CA that issued the server and client certificates, for `NATS_CA`.
    pub fn ca(&self) -> PathBuf {
        self.files.path().join(CA_FILE)
    }

    /// A client certificate the CA issued, for `NATS_CERT`.
    pub fn client_cert(&self) -> PathBuf {
        self.files.path().join(CLIENT_CERT_FILE)
    }

    /// The private key of [`client_cert`](Self::client_cert), for `NATS_KEY`.
    pub fn client_key(&self) -> PathBuf {
        self.files.path().join(CLIENT_KEY_FILE)
    }
}
