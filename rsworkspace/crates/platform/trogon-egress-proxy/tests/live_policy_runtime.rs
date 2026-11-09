#[path = "live_policy_runtime/constants.rs"]
mod constants;
mod support;
#[path = "live_policy_runtime/workload_identity_tests.rs"]
mod workload_identity_tests;

use std::error::Error;
use std::io;
use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::kv;
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;
use trogon_egress_proxy::config::PolicyInput;
use trogon_egress_proxy::fault::Fault;
use trogon_egress_proxy::host_name::HostName;
use trogon_egress_proxy::live_policy::KvPolicyConfig;
use trogon_egress_proxy::resolver::Resolver;
use trogon_egress_proxy::runtime::start_with;
use trogon_nats::NatsConfig;
use trogon_nats::test_support::TlsTestServer;
use trogon_std::env::InMemoryEnv;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use constants::{TICK, WAIT};
use support::{ClientIdentity, Pki, Recorder};

type TestResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

struct NoExternalDns;

#[async_trait]
impl Resolver for NoExternalDns {
    async fn resolve(&self, _: &HostName) -> io::Result<Vec<std::net::IpAddr>> {
        Err(io::ErrorKind::NotFound.into())
    }
}

fn document(generation: u64, policy: &str) -> TestResult<Vec<u8>> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    Ok(serde_json::to_vec(&serde_json::json!({
        "tenant":"acme", "project":"payments", "incarnation":1,
        "generation":generation, "issued_at":now, "valid_until":now+30,
        "policy":policy,
    }))?)
}

async fn request(address: SocketAddr, pki: &Pki, identity: &ClientIdentity) -> TestResult<Vec<u8>> {
    let mut roots = RootCertStore::empty();
    for certificate in rustls_pemfile::certs(&mut pki.ca_pem().as_bytes()) {
        roots.add(certificate?)?;
    }
    let tls = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_client_auth_cert(identity.chain.clone(), identity.key.clone_key())?;
    let mut client = TlsConnector::from(Arc::new(tls))
        .connect(ServerName::try_from("localhost")?, TcpStream::connect(address).await?)
        .await?;
    client
        .write_all(b"CONNECT api.example.com:443 HTTP/1.1\r\n\r\n")
        .await?;
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, client.read_to_end(&mut response)).await??;
    Ok(response)
}

#[tokio::test]
async fn the_production_runtime_starts_and_reloads_through_authenticated_tls_nats() -> TestResult<()> {
    let broker = TlsTestServer::start_jetstream_verifying_clients().await;
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("nats://{}", broker.address()));
    env.set("NATS_CA", broker.ca().to_string_lossy());
    env.set("NATS_CERT", broker.client_cert().to_string_lossy());
    env.set("NATS_KEY", broker.client_key().to_string_lossy());
    let nats = NatsConfig::from_env(&env);
    let writer = trogon_nats::connect(&nats, WAIT).await?;
    tokio::time::timeout(WAIT, writer.flush()).await??;
    let context = async_nats::jetstream::new(writer);
    let dir = tempfile::tempdir()?;
    let pki = Pki::new("runtime-ca");
    let mut config = pki.config(dir.path(), "not a fallback", "127.0.0.1:0".parse()?);
    config.audit.nats = nats;
    let policy = KvPolicyConfig::new(
        "egress_policy",
        config.audit.identity.tenant().clone(),
        config.audit.identity.project().clone(),
        NonZeroU64::MIN,
    )?;
    let store = context
        .create_key_value(kv::Config {
            bucket: policy.bucket().to_owned(),
            max_value_size: 256 * 1024,
            max_bytes: 1024 * 1024,
            max_age: Duration::from_secs(325),
            ..kv::Config::default()
        })
        .await?;
    store
        .put(
            policy.key(),
            document(1, "[agents.agent-1]\nallow=[\"api.example.com:443\"]\n")?.into(),
        )
        .await?;
    config.policy = PolicyInput::NatsKv(policy.clone());
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::INFO);
    let server = start_with(config, Arc::new(NoExternalDns), Arc::new(Recorder::default())).await?;
    let starting = events
        .events()
        .into_iter()
        .find(|event| event.message() == Some("egress proxy starting"))
        .ok_or_else(|| io::Error::other("missing live startup event"))?;
    assert_eq!(starting.field("policy_source"), Some("nats_kv:egress_policy:1"));
    assert_eq!(starting.field("policy_kind"), Some("nats_kv"));
    assert_eq!(starting.field("agents"), Some("1"));
    assert_eq!(starting.field("permits"), Some("1"));
    let address = server.local_addr()?;
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    let identity = pki.agent("agent-1");
    assert_eq!(
        request(address, &pki, &identity).await?,
        Fault::UpstreamUnreachable.response()
    );
    store.put(policy.key(), document(2, "")?.into()).await?;
    tokio::time::timeout(WAIT, async {
        loop {
            let response = request(address, &pki, &identity).await?;
            if response == Fault::NoPermit.response() {
                break TestResult::Ok(());
            }
            assert_eq!(response, Fault::UpstreamUnreachable.response());
            tokio::time::sleep(TICK).await;
        }
    })
    .await??;
    shutdown.cancel();
    task.await?;
    Ok(())
}
