use super::constants::PROBE_TIMEOUT;
use async_nats::{Client, ServerError};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use tempfile::TempDir;
use tokio::sync::mpsc::{self, Receiver};
use trogon_nats::{Location, NKeySeed, NatsAuth, test_support::TlsTestServer};
use trogon_peer_nats::{PeerAccount, PeerCaller, PeerConnectionId, PeerPermissions, PeerPrincipal, PeerTenant};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Serialize)]
struct BrokerPermissions {
    publish: Vec<String>,
    subscribe: Vec<String>,
}
#[derive(Serialize)]
struct BrokerUser {
    nkey: String,
    permissions: BrokerPermissions,
}

pub struct Broker {
    pub server: TlsTestServer,
    files: TempDir,
    seeds: BTreeMap<String, std::path::PathBuf>,
}
impl Broker {
    pub async fn start() -> TestResult<Self> {
        let files = tempfile::tempdir()?;
        let mut seeds = BTreeMap::new();
        let mut users = Vec::new();
        for (identity, caller, connection) in [
            ("service", "service", "service"),
            ("alice", "alice", "one"),
            ("alice_other", "alice", "two"),
            ("bob", "bob", "one"),
        ] {
            let key = nkeys::KeyPair::new_user();
            let path = files.path().join(identity);
            std::fs::write(&path, key.seed()?)?;
            seeds.insert(identity.to_owned(), path);
            let permissions = if identity == "service" {
                BrokerPermissions {
                    publish: vec![">".into()],
                    subscribe: vec![">".into()],
                }
            } else {
                let account = PeerAccount::SharedFixture {
                    tenant: PeerTenant::new("tenant")?,
                };
                let principal = PeerPrincipal::new(PeerTenant::new("tenant")?, PeerCaller::new(caller)?);
                let permissions =
                    PeerPermissions::for_connection(&account, &principal, &PeerConnectionId::new(connection)?)?;
                BrokerPermissions {
                    publish: permissions.publish_allow().to_vec(),
                    subscribe: permissions.subscribe_allow().to_vec(),
                }
            };
            users.push(BrokerUser {
                nkey: key.public_key(),
                permissions,
            });
        }
        let configuration = format!(
            "jetstream {{store_dir: \"/data\"}}\nauthorization {{users: {}}}",
            serde_json::to_string(&users)?
        );
        let server = TlsTestServer::start_with_configuration(&configuration).await;
        Ok(Self { server, files, seeds })
    }
    pub async fn connect(&self, identity: &str) -> TestResult<(Client, Receiver<ServerError>)> {
        let path = self.seeds.get(identity).ok_or("unknown fixture credential")?;
        let auth = NatsAuth::NKey(NKeySeed::File(Location::Path(path.clone())));
        let (send, receive) = mpsc::channel(64);
        let send = Arc::new(send);
        let options = self
            .server
            .connect_options(auth)
            .await?
            .connection_timeout(PROBE_TIMEOUT)
            .event_callback(move |event| {
                let send = send.clone();
                async move {
                    if let async_nats::Event::ServerError(error) = event {
                        let _ = send.try_send(error);
                    }
                }
            });
        let client = options.connect(self.server.address()).await?;
        assert!(client.server_info().tls_required);
        Ok((client, receive))
    }
    pub fn credentials_are_ephemeral(&self) -> bool {
        self.files.path().exists()
    }
}
