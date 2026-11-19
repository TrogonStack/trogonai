use std::collections::HashMap;
use std::time::Duration;

use async_nats::jetstream::{
    Error as JetStreamError, ErrorCode,
    context::RequestErrorKind,
    stream::{ClusterInfo, Config, InfoError, StorageType},
};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair};
use serde_json::{Value, json};
use tempfile::TempDir;
use testcontainers_modules::testcontainers::ImageExt;
use tokio::time::timeout;
use uuid::Uuid;

use super::{IssuedCertificate, SERVER_NAMES, TestServer, image};
use crate::{ClientCertificate, Location, NatsAuth, NatsConfig, NatsTls, NatsToken, connect_once};

mod constants;
mod quorum;

#[cfg(test)]
mod tests;

use constants::{ADMIN, NATS_CLUSTER_IMAGE_TAG, QUORUM_STREAM, WAIT};
use quorum::wait_for_quorum;

/// A three-node file-backed JetStream cluster. Client certificates are mapped
/// to the supplied native NATS permission objects. Routes have a separate CA.
pub struct TlsJetStreamCluster {
    nodes: Vec<TestServer>,
    files: TempDir,
    identities: HashMap<NatsToken, ClientCertificate>,
}

impl TlsJetStreamCluster {
    pub async fn start(users: &[(NatsToken, Value)]) -> Self {
        let files = tempfile::tempdir().expect("create TLS cluster certificate directory");
        let network = format!("nats-tls-{}", Uuid::now_v7().simple());
        let names: Vec<_> = (0..3).map(|index| format!("{network}-{index}")).collect();

        let mut client_ca = CertificateParams::default();
        client_ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let client_ca = CertifiedIssuer::self_signed(client_ca, KeyPair::generate().expect("client CA key"))
            .expect("client CA certificate");
        let mut route_ca = CertificateParams::default();
        route_ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let route_ca = CertifiedIssuer::self_signed(route_ca, KeyPair::generate().expect("route CA key"))
            .expect("route CA certificate");
        let client_ca_pem = client_ca.pem();
        let route_ca_pem = route_ca.pem();
        std::fs::write(files.path().join("ca.pem"), &client_ca_pem).expect("write cluster client CA");

        let admin = NatsToken::new(ADMIN).expect("fixture administrator name");
        assert!(
            !users.iter().any(|(name, _)| name == &admin),
            "reserved fixture identity"
        );
        let all_users = std::iter::once((admin, json!({"publish": ["$JS.API.>"], "subscribe": ["_INBOX.>"]})))
            .chain(users.iter().cloned());
        let mut identities = HashMap::new();
        let mut authorization = Vec::new();
        for (index, (name, permissions)) in all_users.enumerate() {
            let mut params = CertificateParams::default();
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
            let certificate = IssuedCertificate::signed_by(params, name.as_str(), &client_ca);
            let cert = files.path().join(format!("client-{index}.pem"));
            let key = files.path().join(format!("client-{index}.key"));
            std::fs::write(&cert, certificate.cert).expect("write cluster user certificate");
            std::fs::write(&key, certificate.key).expect("write cluster user key");
            assert!(
                identities
                    .insert(
                        name.clone(),
                        ClientCertificate {
                            cert: Location::Path(cert),
                            key: Location::Path(key),
                        }
                    )
                    .is_none(),
                "duplicate fixture identity"
            );
            authorization.push(json!({"user": format!("CN={name}"), "permissions": permissions}));
        }

        let mut nodes = Vec::new();
        for name in &names {
            let mut client_params =
                CertificateParams::new(SERVER_NAMES.map(String::from)).expect("cluster server certificate names");
            client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            let client = IssuedCertificate::signed_by(client_params, name, &client_ca);
            let mut route_params = CertificateParams::new(vec![name.clone()]).expect("route certificate name");
            route_params.extended_key_usages =
                vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
            let route = IssuedCertificate::signed_by(route_params, name, &route_ca);
            let routes: Vec<_> = names
                .iter()
                .filter(|peer| *peer != name)
                .map(|peer| format!("nats://{peer}:6222"))
                .collect();
            let config = json!({
                "server_name": name,
                "listen": "0.0.0.0:4222",
                "jetstream": {"store_dir": "/data"},
                "tls": {
                    "cert_file": "/tls/client-server.pem", "key_file": "/tls/client-server.key",
                    "ca_file": "/tls/client-ca.pem", "verify_and_map": true, "min_version": "1.3"
                },
                "authorization": {"users": authorization},
                "cluster": {
                    "name": network, "listen": "0.0.0.0:6222", "routes": routes,
                    "tls": {
                        "cert_file": "/tls/route.pem", "key_file": "/tls/route.key",
                        "ca_file": "/tls/route-ca.pem", "verify": true, "min_version": "1.3"
                    }
                }
            });
            let request = image(["--config", "/nats.conf"])
                .with_tag(NATS_CLUSTER_IMAGE_TAG)
                .with_network(&network)
                .with_container_name(name)
                .with_copy_to(
                    "/nats.conf",
                    serde_json::to_vec(&config).expect("encode cluster configuration"),
                )
                .with_copy_to("/tls/client-ca.pem", client_ca_pem.as_bytes().to_vec())
                .with_copy_to("/tls/client-server.pem", client.cert.into_bytes())
                .with_copy_to("/tls/client-server.key", client.key.into_bytes())
                .with_copy_to("/tls/route-ca.pem", route_ca_pem.as_bytes().to_vec())
                .with_copy_to("/tls/route.pem", route.cert.into_bytes())
                .with_copy_to("/tls/route.key", route.key.into_bytes());
            nodes.push(TestServer::start(request).await);
        }
        let cluster = Self {
            nodes,
            files,
            identities,
        };
        cluster.require_quorum().await;
        cluster
    }

    pub fn config(&self, identity: &NatsToken) -> Option<NatsConfig> {
        let certificate = self.identities.get(identity)?.clone();
        Some(
            NatsConfig::new(
                self.nodes.iter().map(|node| node.address.clone()).collect(),
                NatsAuth::None,
            )
            .with_tls(NatsTls::Enabled {
                ca: Some(Location::Path(self.files.path().join("ca.pem"))),
                client_cert: Some(certificate),
            }),
        )
    }

    async fn require_quorum(&self) {
        let config = self
            .config(&NatsToken::new(ADMIN).expect("fixture administrator name"))
            .expect("fixture administrator certificate");
        let client = connect_once(&config, config.auth.clone(), Duration::from_secs(5))
            .await
            .expect("connect to verified TLS cluster");
        let context = async_nats::jetstream::new(client);
        timeout(WAIT, async {
            wait_for_quorum(async || {
                let stream = context
                    .create_stream(Config {
                        name: QUORUM_STREAM.into(),
                        subjects: vec!["fixture.quorum".into()],
                        num_replicas: 3,
                        storage: StorageType::File,
                        max_messages: 1,
                        ..Config::default()
                    })
                    .await
                    .ok()?;
                quorum_observation(stream.get_info().await.map(|info| info.cluster))
            })
            .await;
            context
                .delete_stream(QUORUM_STREAM)
                .await
                .expect("remove quorum probe stream");
        })
        .await
        .expect("three verified TLS JetStream replicas become current");
    }
}

fn quorum_observation(result: Result<Option<ClusterInfo>, InfoError>) -> Option<ClusterInfo> {
    if result.as_ref().is_err_and(|error| {
        error.kind() == RequestErrorKind::Other
            && std::error::Error::source(error)
                .and_then(|source| source.downcast_ref::<JetStreamError>())
                .is_some_and(|source| source.error_code() == ErrorCode::STREAM_NOT_FOUND)
    }) {
        return None;
    }
    result.expect("inspect quorum probe stream")
}
