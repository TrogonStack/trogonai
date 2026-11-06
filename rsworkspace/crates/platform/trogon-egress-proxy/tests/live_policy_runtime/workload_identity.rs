use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogon_audit::{ActivationRecorder, Actor, AuditWriter, JetStreamSink, RecordScope, Stamper, WriterConfig, WriterId, WriterIdentity};
use trogon_egress_proxy::config::{Config, TlsInput};
use trogon_egress_proxy::limits::Limits;
use trogon_egress_proxy::runtime::start;
use trogon_egress_proxy::trust_domain::TrustDomain;
use trogon_egress_proxy::workload_identity_config::WorkloadIdentityConfig;
use trogon_egress_proxy::workload_activation_audit::WorkloadActivationAudit;
use trogon_egress_proxy::workload_registration::decode_registration;
use trogon_std::SystemClock;
use trogon_std::uuid::UuidV7Generator;
use trogon_nats::NatsToken;
use trogon_workload_identity::{IdentityConfig, IdentityStore, RegistrationBinding as RuntimeBinding, WorkloadRole as RuntimeRole};
use trogon_workload_identity::test_support::{TestCa, WorkloadApiFixture};
use trogonai_proto::platform::audit::v1::{AuditRecord, audit_record::Detail};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

use super::constants::{APPLICATION_ID, LOCAL_WORKLOAD, PEER_WORKLOAD};
use super::*;

async fn exclusive_snapshot(context: &async_nats::jetstream::Context, store: &kv::Store, key: &str) -> TestResult<()> {
    let mut config = store.stream.get_info().await?.config;
    config.subjects = vec![format!("$KV.{}.{}", store.name, key)];
    config.max_messages = 1;
    config.discard = async_nats::jetstream::stream::DiscardPolicy::Old;
    context.update_stream(config).await?;
    Ok(())
}

#[tokio::test]
async fn production_workload_profile_authenticates_and_audits_without_static_tls_files() -> TestResult<()> {
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
    policies
        .put(
            policy.key(),
            document(
                1,
                &format!("[agents.\"{APPLICATION_ID}\"]\nallow=[\"api.example.com:443\"]\n"),
            )?
            .into(),
        )
        .await?;
    let client_api = WorkloadApiFixture::start(vec![peer]).await;
    let client_config = IdentityConfig::new(
        client_api.endpoint(),
        RuntimeBinding::new(PEER_WORKLOAD, "acme", "payments", RuntimeRole::Agent, Some(APPLICATION_ID))?,
        NonZeroU64::MIN,
    );
    let client_writer_identity = WriterIdentity::new(
        WriterId::new("workload-client:fixture")?, NatsToken::new("workload_client")?,
        audit.identity.tenant().clone(), audit.identity.project().clone(),
    );
    let client_writer = AuditWriter::new(
        JetStreamSink::with_defaults(context.clone()), client_writer_identity, WriterConfig::default(),
        &Stamper::new(SystemClock, UuidV7Generator),
    );
    let client_audit = WorkloadActivationAudit::new(ActivationRecorder::new(client_writer.clone(), RecordScope::new(
        audit.identity.tenant().clone(), audit.identity.project().clone(), Actor::Unattributed,
    )));
    let (client_identity, client_driver) = IdentityStore::new(client_config.clone(), Arc::new(client_audit));
    let registration = registry.stream.get_last_raw_message_by_subject(&format!("$KV.{}.{}", registry.name, workload.registration_key())).await?;
    client_identity.replace_registration(decode_registration(&registration.payload, &client_config, SystemTime::now())?)?;
    let client_shutdown = CancellationToken::new();
    let client_token = client_shutdown.clone();
    let client_task = tokio::spawn(async move { client_driver.run(client_token).await });
    let client_audit_token = client_shutdown.clone();
    let client_audit_task = tokio::spawn(async move { client_writer.run(client_audit_token).await });
    let client_material = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(material) = client_identity.material() { return material; }
            tokio::time::sleep(TICK).await;
        }
    }).await?;
    let expected_server = RuntimeBinding::new(LOCAL_WORKLOAD, "acme", "payments", RuntimeRole::EgressProxy, None)?;
    let connector = TlsConnector::from(client_material.client_config(expected_server.spiffe_id())?);
    let server = start(Config {
        listen: "127.0.0.1:0".parse()?,
        policy: PolicyInput::NatsKv(policy),
        tls: TlsInput::Workload(workload),
        trust_domain,
        audit,
        non_tls_ports: Vec::new(),
        limits: Limits::default(),
    })
    .await?;
    let address = server.local_addr()?;
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    let mut last_handshake_error = None;
    let request = tokio::time::timeout(WAIT, async {
        loop {
            let transport = TcpStream::connect(address).await?;
            match connector.connect(ServerName::try_from("localhost")?, transport).await {
                Ok(mut client) => {
                    client.write_all(b"GET / HTTP/1.1\r\n\r\n").await?;
                    let mut reply = Vec::new();
                    client.read_to_end(&mut reply).await?;
                    assert_eq!(reply, Fault::MethodNotAllowed.response());
                    return TestResult::Ok(());
                },
                Err(error) => last_handshake_error = Some(error),
            }
            tokio::time::sleep(TICK).await;
        }
    })
    .await;
    match request {
        Ok(outcome) => outcome?,
        Err(elapsed) => {
            if let Some(error) = last_handshake_error { return Err(error.into()); }
            return Err(elapsed.into());
        },
    }
    assert!(workload_api.requests() > 0);
    let stored = audit_stream
        .get_last_raw_message_by_subject("platform.v1.audit.change.payments.credential")
        .await?;
    let (body, _) = trogon_audit::record_seal::open(&stored.payload)?;
    let activation = AuditRecord::decode_from_slice(body)?;
    assert!(
        matches!(activation.detail, Some(Detail::CredentialActivation(fact)) if fact.binding.spiffe_id == LOCAL_WORKLOAD && fact.registration_revision == 1)
    );
    shutdown.cancel();
    task.await?;
    client_shutdown.cancel();
    client_task.await?;
    client_audit_task.await?;
    client_api.stop().await;
    workload_api.stop().await;
    Ok(())
}
