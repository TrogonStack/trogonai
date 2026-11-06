use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogon_audit::{
    ActivationRecorder, Actor, AuditWriter, JetStreamSink, RecordScope, Stamper, WriterConfig, WriterId, WriterIdentity,
};
use trogon_egress_proxy::config::{Config, TlsInput};
use trogon_egress_proxy::limits::Limits;
use trogon_egress_proxy::runtime::start;
use trogon_egress_proxy::trust_domain::TrustDomain;
use trogon_egress_proxy::workload_activation_audit::WorkloadActivationAudit;
use trogon_egress_proxy::workload_identity_config::WorkloadIdentityConfig;
use trogon_egress_proxy::workload_registration::decode_registration;
use trogon_nats::NatsToken;
use trogon_std::SystemClock;
use trogon_std::uuid::UuidV7Generator;
use trogon_workload_identity::test_support::{TestCa, WorkloadApiFixture};
use trogon_workload_identity::{
    IdentityConfig, IdentityStore, RegistrationBinding as RuntimeBinding, WorkloadRole as RuntimeRole,
};
use trogonai_proto::platform::audit::v1::{AuditRecord, audit_record::Detail};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

use super::constants::{APPLICATION_ID, LOCAL_WORKLOAD, PEER_WORKLOAD};
use super::*;

#[path = "readiness.rs"]
mod readiness;

async fn exclusive_snapshot(context: &async_nats::jetstream::Context, store: &kv::Store, key: &str) -> TestResult<()> {
    let mut config = store.stream.get_info().await?.config;
    config.subjects = vec![format!("$KV.{}.{}", store.name, key)];
    config.max_messages = 1;
    config.discard = async_nats::jetstream::stream::DiscardPolicy::Old;
    context.update_stream(config).await?;
    Ok(())
}

struct WorkloadHarness {
    readiness: trogon_egress_proxy::Readiness,
    _broker: TlsTestServer,
    context: async_nats::jetstream::Context,
    audit_stream: async_nats::jetstream::stream::Stream,
    registry: kv::Store,
    policies: kv::Store,
    registration_key: String,
    policy_key: String,
    workload_api: WorkloadApiFixture,
    client_api: WorkloadApiFixture,
    client_identity: IdentityStore,
    connector: TlsConnector,
    address: SocketAddr,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    client_shutdown: CancellationToken,
    client_task: tokio::task::JoinHandle<()>,
    client_audit_task: tokio::task::JoinHandle<()>,
}

impl WorkloadHarness {
    async fn start(policy_text: &str) -> TestResult<Self> {
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
        let audit_stream = context
            .create_stream(async_nats::jetstream::stream::Config {
                name: trogon_audit::constants::STREAM_AUDIT.into(),
                subjects: vec!["platform.v1.audit.>".into()],
                max_bytes: 1024 * 1024,
                discard: async_nats::jetstream::stream::DiscardPolicy::New,
                ..Default::default()
            })
            .await?;
        let ca = TestCa::new();
        let local = ca.issue(LOCAL_WORKLOAD, Duration::from_secs(120));
        let peer = ca.issue(PEER_WORKLOAD, Duration::from_secs(120));
        let workload_api = WorkloadApiFixture::start(vec![local]).await;
        let mut audit = support::audit_config();
        audit.nats = nats;
        let trust_domain = TrustDomain::new("trogon.example")?;
        let workload = WorkloadIdentityConfig::new(
            &workload_api.endpoint().as_uri(),
            LOCAL_WORKLOAD,
            "WORKLOAD_REGISTRATIONS",
            NonZeroU64::MIN,
            &trust_domain,
            &audit,
        )?;
        let registry = context
            .create_key_value(kv::Config {
                bucket: workload.registration_bucket().into(),
                max_value_size: 256 * 1024,
                max_bytes: 1024 * 1024,
                max_age: Duration::from_secs(325),
                ..Default::default()
            })
            .await?;
        exclusive_snapshot(&context, &registry, workload.registration_key()).await?;
        let now = SystemTime::now();
        registry
            .put(
                workload.registration_key(),
                RegistrationSnapshot {
                    tenant: "acme".into(),
                    project: "payments".into(),
                    recovery_fence: 1,
                    registration_revision: 1,
                    trust_revision: 1,
                    issued_at: MessageField::some(Timestamp::from(now)),
                    expires_at: MessageField::some(Timestamp::from(now + Duration::from_secs(60))),
                    bindings: vec![
                        RegistrationBinding {
                            spiffe_id: LOCAL_WORKLOAD.into(),
                            tenant: "acme".into(),
                            project: "payments".into(),
                            role: EnumValue::from(WorkloadRole::EgressProxy),
                            application_id: None,
                        },
                        RegistrationBinding {
                            spiffe_id: PEER_WORKLOAD.into(),
                            tenant: "acme".into(),
                            project: "payments".into(),
                            role: EnumValue::from(WorkloadRole::Agent),
                            application_id: Some(APPLICATION_ID.into()),
                        },
                    ],
                    approved_root_sha256: vec![ca.root_sha256().to_vec()],
                }
                .encode_to_vec()
                .into(),
            )
            .await?;
        let policy = KvPolicyConfig::new(
            "WORKLOAD_EGRESS_POLICY",
            audit.identity.tenant().clone(),
            audit.identity.project().clone(),
            NonZeroU64::MIN,
        )?;
        let policies = context
            .create_key_value(kv::Config {
                bucket: policy.bucket().into(),
                max_value_size: 256 * 1024,
                max_bytes: 1024 * 1024,
                max_age: Duration::from_secs(325),
                ..Default::default()
            })
            .await?;
        exclusive_snapshot(&context, &policies, policy.key()).await?;
        policies.put(policy.key(), document(1, policy_text)?.into()).await?;
        let client_api = WorkloadApiFixture::start(vec![peer]).await;
        let client_config = IdentityConfig::new(
            client_api.endpoint(),
            RuntimeBinding::new(
                PEER_WORKLOAD,
                "acme",
                "payments",
                RuntimeRole::Agent,
                Some(APPLICATION_ID),
            )?,
            NonZeroU64::MIN,
        );
        let client_writer_identity = WriterIdentity::new(
            WriterId::new("workload-client:fixture")?,
            NatsToken::new("workload_client")?,
            audit.identity.tenant().clone(),
            audit.identity.project().clone(),
        );
        let client_writer = AuditWriter::new(
            JetStreamSink::with_defaults(context.clone()),
            client_writer_identity,
            WriterConfig::default(),
            &Stamper::new(SystemClock, UuidV7Generator),
        );
        let client_audit = WorkloadActivationAudit::new(ActivationRecorder::new(
            client_writer.clone(),
            RecordScope::new(
                audit.identity.tenant().clone(),
                audit.identity.project().clone(),
                Actor::Unattributed,
            ),
        ));
        let (client_identity, client_driver) = IdentityStore::new(client_config.clone(), Arc::new(client_audit));
        let registration = registry
            .stream
            .get_last_raw_message_by_subject(&format!("$KV.{}.{}", registry.name, workload.registration_key()))
            .await?;
        client_identity.replace_registration(decode_registration(
            &registration.payload,
            &client_config,
            SystemTime::now(),
        )?)?;
        let client_shutdown = CancellationToken::new();
        let client_token = client_shutdown.clone();
        let client_task = tokio::spawn(async move { client_driver.run(client_token).await });
        let client_audit_token = client_shutdown.clone();
        let client_audit_task = tokio::spawn(async move { client_writer.run(client_audit_token).await });
        let client_material = tokio::time::timeout(WAIT, async {
            loop {
                if let Some(material) = client_identity.material() {
                    return material;
                }
                tokio::time::sleep(TICK).await;
            }
        })
        .await?;
        let expected_server = RuntimeBinding::new(LOCAL_WORKLOAD, "acme", "payments", RuntimeRole::EgressProxy, None)?;
        let connector = TlsConnector::from(client_material.client_config(expected_server.spiffe_id())?);
        let server = start(Config {
            listen: "127.0.0.1:0".parse()?,
            policy: PolicyInput::NatsKv(policy.clone()),
            tls: TlsInput::Workload(workload.clone()),
            trust_domain,
            audit,
            non_tls_ports: Vec::new(),
            limits: Limits::default(),
        })
        .await?;
        let readiness = server.readiness();
        let address = server.local_addr()?;
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(server.run(shutdown.clone()));
        Ok(Self {
            readiness,
            _broker: broker,
            context,
            audit_stream,
            registry,
            policies,
            registration_key: workload.registration_key().into(),
            policy_key: policy.key().into(),
            workload_api,
            client_api,
            client_identity,
            connector,
            address,
            shutdown,
            task,
            client_shutdown,
            client_task,
            client_audit_task,
        })
    }

    async fn connect(&self) -> TestResult<tokio_rustls::client::TlsStream<TcpStream>> {
        tokio::time::timeout(WAIT, async {
            loop {
                let transport = TcpStream::connect(self.address).await?;
                match self
                    .connector
                    .connect(ServerName::try_from("localhost")?, transport)
                    .await
                {
                    Ok(client) => return TestResult::Ok(client),
                    Err(_) => tokio::time::sleep(TICK).await,
                }
            }
        })
        .await?
    }

    async fn method_request(&self) -> TestResult<Vec<u8>> {
        let transport = TcpStream::connect(self.address).await?;
        let mut client = self
            .connector
            .connect(ServerName::try_from("localhost")?, transport)
            .await?;
        client.write_all(b"GET / HTTP/1.1\r\n\r\n").await?;
        let mut reply = Vec::new();
        tokio::time::timeout(WAIT, client.read_to_end(&mut reply)).await??;
        Ok(reply)
    }

    async fn forwarding(&self, port: u16) -> TestResult<tokio_rustls::client::TlsStream<TcpStream>> {
        let mut client = self.connect().await?;
        client
            .write_all(format!("CONNECT localhost:{port} HTTP/1.1\r\n\r\n").as_bytes())
            .await?;
        assert_eq!(
            support::read_head(&mut client).await,
            "HTTP/1.1 200 Connection Established\r\n\r\n"
        );
        let hello = support::client_hello("localhost");
        client.write_all(&hello).await?;
        assert_eq!(support::read_exactly(&mut client, hello.len()).await, hello);
        Ok(client)
    }

    async fn snapshot(&self) -> TestResult<RegistrationSnapshot> {
        let stored = self
            .registry
            .stream
            .get_last_raw_message_by_subject(&format!("$KV.{}.{}", self.registry.name, self.registration_key))
            .await?;
        Ok(RegistrationSnapshot::decode_from_slice(&stored.payload)?)
    }

    async fn publish_registration(&self, snapshot: RegistrationSnapshot) -> TestResult<()> {
        self.registry
            .put(&self.registration_key, snapshot.encode_to_vec().into())
            .await?;
        Ok(())
    }

    async fn decision(&self, terminal: &str) -> TestResult<AuditRecord> {
        let stored = self
            .audit_stream
            .get_last_raw_message_by_subject(&format!("platform.v1.audit.decision.payments.{terminal}"))
            .await?;
        let (body, _) = trogon_audit::record_seal::open(&stored.payload)?;
        Ok(AuditRecord::decode_from_slice(body)?)
    }

    async fn stop(self) -> TestResult<()> {
        self.shutdown.cancel();
        self.task.await?;
        self.client_shutdown.cancel();
        self.client_task.await?;
        self.client_audit_task.await?;
        self.client_api.stop().await;
        self.workload_api.stop().await;
        Ok(())
    }
}

#[tokio::test]
async fn production_workload_profile_authenticates_and_audits_without_static_tls_files() -> TestResult<()> {
    let harness = WorkloadHarness::start(&format!(
        "[agents.\"{APPLICATION_ID}\"]\nallow=[\"api.example.com:443\"]\n"
    ))
    .await?;
    let mut client = harness.connect().await?;
    client.write_all(b"GET / HTTP/1.1\r\n\r\n").await?;
    let mut reply = Vec::new();
    tokio::time::timeout(WAIT, client.read_to_end(&mut reply)).await??;
    assert_eq!(reply, Fault::MethodNotAllowed.response());
    assert!(harness.workload_api.requests() > 0);
    let stored = harness
        .audit_stream
        .get_last_raw_message_by_subject("platform.v1.audit.change.payments.credential")
        .await?;
    let (body, _) = trogon_audit::record_seal::open(&stored.payload)?;
    let activation = AuditRecord::decode_from_slice(body)?;
    assert!(
        matches!(activation.detail, Some(Detail::CredentialActivation(fact)) if fact.binding.spiffe_id == LOCAL_WORKLOAD && fact.registration_revision == 1)
    );
    let decision = harness.decision("deny").await?;
    let Some(Detail::GatewayDecision(decision)) = decision.detail else {
        return Err(io::Error::other("missing decision").into());
    };
    let peer = decision
        .workload_peer
        .as_option()
        .ok_or_else(|| io::Error::other("missing authenticated peer evidence"))?;
    assert_eq!(peer.peer_binding.spiffe_id, PEER_WORKLOAD);
    assert_eq!(peer.peer_binding.application_id.as_deref(), Some(APPLICATION_ID));
    assert_eq!(peer.registration_revision, 1);
    assert_eq!(peer.peer_leaf_sha256.len(), 32);
    harness.stop().await
}

fn localhost_policy(port: u16) -> String {
    format!("[agents.\"{APPLICATION_ID}\"]\nallow=[\"localhost:{port}\",\"127.0.0.1:{port}\"]\n")
}

async fn closed<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> TestResult<()> {
    let mut byte = [0u8; 1];
    match tokio::time::timeout(WAIT, stream.read(&mut byte)).await? {
        Ok(0) | Err(_) => Ok(()),
        Ok(_) => Err(io::Error::other("authority withdrawal left application bytes readable").into()),
    }
}

#[tokio::test]
async fn production_registration_removal_closes_idle_and_forwarding_workload_sessions() -> TestResult<()> {
    let upstream = support::Upstream::start().await;
    let harness = WorkloadHarness::start(&localhost_policy(upstream.port)).await?;
    let mut active = harness.forwarding(upstream.port).await?;
    let mut idle = harness.connect().await?;
    active.write_all(b"approved").await?;
    assert_eq!(support::read_exactly(&mut active, 8).await, b"approved");
    let allow = harness.decision("allow").await?;
    let Some(Detail::GatewayDecision(decision)) = allow.detail else {
        return Err(io::Error::other("missing decision").into());
    };
    assert_eq!(
        decision.workload_peer.peer_binding.application_id.as_deref(),
        Some(APPLICATION_ID)
    );
    let mut revoked = harness.snapshot().await?;
    revoked.registration_revision = 2;
    revoked.bindings.retain(|binding| binding.spiffe_id != PEER_WORKLOAD);
    harness.publish_registration(revoked).await?;
    closed(&mut active).await?;
    closed(&mut idle).await?;
    assert!(harness.method_request().await.is_err());
    harness.stop().await
}

#[tokio::test]
async fn production_source_permission_denied_closes_sessions_before_another_http_request() -> TestResult<()> {
    let harness = WorkloadHarness::start("").await?;
    let mut idle = harness.connect().await?;
    harness.workload_api.permission_denied();
    closed(&mut idle).await?;
    assert!(harness.method_request().await.is_err());
    harness.stop().await
}

#[tokio::test]
async fn production_policy_withdrawal_closes_an_established_workload_tunnel() -> TestResult<()> {
    let upstream = support::Upstream::start().await;
    let harness = WorkloadHarness::start(&localhost_policy(upstream.port)).await?;
    let mut active = harness.forwarding(upstream.port).await?;
    harness
        .policies
        .put(&harness.policy_key, document(2, "")?.into())
        .await?;
    closed(&mut active).await?;
    harness.stop().await
}

#[tokio::test]
async fn production_audit_outage_refuses_a_new_workload_tunnel_before_forwarding() -> TestResult<()> {
    let upstream = support::Upstream::start().await;
    let harness = WorkloadHarness::start(&localhost_policy(upstream.port)).await?;
    let mut first = harness.forwarding(upstream.port).await?;
    first.shutdown().await?;
    let recorded_bytes = upstream.received();
    harness
        .context
        .delete_stream(trogon_audit::constants::STREAM_AUDIT)
        .await?;
    let mut client = harness.connect().await?;
    let mut request = format!("CONNECT localhost:{} HTTP/1.1\r\n\r\n", upstream.port).into_bytes();
    request.extend_from_slice(&support::client_hello("localhost"));
    client.write_all(&request).await?;
    let mut reply = Vec::new();
    tokio::time::timeout(WAIT, client.read_to_end(&mut reply)).await??;
    assert_eq!(reply, Fault::AuditUnavailable.response());
    assert_eq!(upstream.received(), recorded_bytes);
    harness.stop().await
}

#[tokio::test]
async fn production_registration_stream_scope_widening_withdraws_workload_authority() -> TestResult<()> {
    let harness = WorkloadHarness::start("").await?;
    let mut idle = harness.connect().await?;
    let mut stream = harness.registry.stream.get_info().await?.config;
    stream
        .subjects
        .push(format!("$KV.{}.another_scope", harness.registry.name));
    harness.context.update_stream(stream).await?;
    closed(&mut idle).await?;
    assert!(harness.method_request().await.is_err());
    harness.stop().await
}

#[tokio::test]
async fn production_trust_replacement_retires_old_sessions_and_accepts_only_new_material() -> TestResult<()> {
    let mut harness = WorkloadHarness::start("").await?;
    let mut old_session = harness.connect().await?;
    let old_connector = harness.connector.clone();
    let new_ca = TestCa::new();
    let local = new_ca.issue(LOCAL_WORKLOAD, Duration::from_secs(120));
    let peer = new_ca.issue(PEER_WORKLOAD, Duration::from_secs(120));
    let mut registration = harness.snapshot().await?;
    registration.registration_revision = 2;
    registration.trust_revision = 2;
    registration.approved_root_sha256 = vec![new_ca.root_sha256().to_vec()];
    harness.publish_registration(registration.clone()).await?;
    closed(&mut old_session).await?;
    harness.client_identity.replace_registration(decode_registration(
        &registration.encode_to_vec(),
        harness.client_identity.config(),
        SystemTime::now(),
    )?)?;
    harness.workload_api.replace(vec![local]);
    harness.client_api.replace(vec![peer]);
    let new_material = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(material) = harness.client_identity.material()
                && material.fact().registration_revision().get() == 2
            {
                return material;
            }
            tokio::time::sleep(TICK).await;
        }
    })
    .await?;
    let expected = RuntimeBinding::new(LOCAL_WORKLOAD, "acme", "payments", RuntimeRole::EgressProxy, None)?;
    harness.connector = TlsConnector::from(new_material.client_config(expected.spiffe_id())?);
    let mut new_session = harness.connect().await?;
    new_session.write_all(b"GET / HTTP/1.1\r\n\r\n").await?;
    let mut reply = Vec::new();
    tokio::time::timeout(WAIT, new_session.read_to_end(&mut reply)).await??;
    assert_eq!(reply, Fault::MethodNotAllowed.response());
    let old = old_connector
        .connect(
            ServerName::try_from("localhost")?,
            TcpStream::connect(harness.address).await?,
        )
        .await;
    assert!(old.is_err(), "a cached TLS config must lose its retired authority");
    let decision = harness.decision("deny").await?;
    let Some(Detail::GatewayDecision(decision)) = decision.detail else {
        return Err(io::Error::other("missing decision").into());
    };
    assert_eq!(decision.workload_peer.registration_revision, 2);
    assert_eq!(decision.workload_peer.trust_revision, 2);
    harness.stop().await
}
