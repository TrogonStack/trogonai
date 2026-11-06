use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogon_egress_proxy::config::{Config, TlsInput};
use trogon_egress_proxy::limits::Limits;
use trogon_egress_proxy::runtime::start;
use trogon_egress_proxy::trust_domain::TrustDomain;
use trogon_egress_proxy::workload_identity_config::WorkloadIdentityConfig;
use trogon_workload_identity::test_support::{TestCa, WorkloadApiFixture};
use trogonai_proto::platform::audit::v1::{AuditRecord, audit_record::Detail};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

use super::constants::{APPLICATION_ID, LOCAL_WORKLOAD, PEER_WORKLOAD};
use super::*;

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
    let audit_stream = context.create_stream(async_nats::jetstream::stream::Config {
        name: trogon_audit::constants::STREAM_AUDIT.into(),
        subjects: vec!["platform.v1.audit.>".into()],
        max_bytes: 1024 * 1024,
        discard: async_nats::jetstream::stream::DiscardPolicy::New,
        ..Default::default()
    }).await?;
    let ca = TestCa::new();
    let local = ca.issue(LOCAL_WORKLOAD, Duration::from_secs(120));
    let peer = ca.issue(PEER_WORKLOAD, Duration::from_secs(120));
    let workload_api = WorkloadApiFixture::start(vec![local]).await;
    let mut audit = support::audit_config();
    audit.nats = nats;
    let trust_domain = TrustDomain::new("trogon.example")?;
    let workload = WorkloadIdentityConfig::new(
        &workload_api.endpoint().as_uri(), LOCAL_WORKLOAD, "WORKLOAD_REGISTRATIONS",
        NonZeroU64::MIN, &trust_domain, &audit,
    )?;
    let registry = context.create_key_value(kv::Config {
        bucket: workload.registration_bucket().into(),
        max_value_size: 256 * 1024,
        max_bytes: 1024 * 1024,
        max_age: Duration::from_secs(325),
        ..Default::default()
    }).await?;
    let now = SystemTime::now();
    registry.put(workload.registration_key(), RegistrationSnapshot {
        tenant: "acme".into(), project: "payments".into(),
        recovery_fence: 1, registration_revision: 1, trust_revision: 1,
        issued_at: MessageField::some(Timestamp::from(now)),
        expires_at: MessageField::some(Timestamp::from(now + Duration::from_secs(60))),
        bindings: vec![
            RegistrationBinding { spiffe_id: LOCAL_WORKLOAD.into(), tenant: "acme".into(), project: "payments".into(), role: EnumValue::from(WorkloadRole::EgressProxy), application_id: None },
            RegistrationBinding { spiffe_id: PEER_WORKLOAD.into(), tenant: "acme".into(), project: "payments".into(), role: EnumValue::from(WorkloadRole::Agent), application_id: Some(APPLICATION_ID.into()) },
        ],
        approved_root_sha256: vec![ca.root_sha256().to_vec()],
    }.encode_to_vec().into()).await?;
    let policy = KvPolicyConfig::new("WORKLOAD_EGRESS_POLICY", audit.identity.tenant().clone(), audit.identity.project().clone(), NonZeroU64::MIN)?;
    let policies = context.create_key_value(kv::Config {
        bucket: policy.bucket().into(), max_value_size: 256 * 1024,
        max_bytes: 1024 * 1024, max_age: Duration::from_secs(325), ..Default::default()
    }).await?;
    policies.put(policy.key(), document(1, &format!("[agents.\"{APPLICATION_ID}\"]\nallow=[\"api.example.com:443\"]\n"))?.into()).await?;
    let server = start(Config {
        listen: "127.0.0.1:0".parse()?, policy: PolicyInput::NatsKv(policy),
        tls: TlsInput::Workload(workload), trust_domain, audit,
        non_tls_ports: Vec::new(), limits: Limits::default(),
    }).await?;
    let address = server.local_addr()?;
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    let mut roots = RootCertStore::empty();
    roots.add(ca.root())?;
    let client = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()?.with_root_certificates(roots)
        .with_client_auth_cert(peer.certificates(), peer.private_key())?;
    let connector = TlsConnector::from(Arc::new(client));
    tokio::time::timeout(WAIT, async {
        loop {
            let transport = TcpStream::connect(address).await?;
            if let Ok(mut client) = connector.connect(ServerName::try_from("localhost")?, transport).await {
                client.write_all(b"GET / HTTP/1.1\r\n\r\n").await?;
                let mut reply = Vec::new();
                client.read_to_end(&mut reply).await?;
                assert_eq!(reply, Fault::MethodNotAllowed.response());
                return TestResult::Ok(());
            }
            tokio::time::sleep(TICK).await;
        }
    }).await??;
    assert!(workload_api.requests() > 0);
    let stored = audit_stream.get_last_raw_message_by_subject("platform.v1.audit.change.payments.credential").await?;
    let (body, _) = trogon_audit::record_seal::open(&stored.payload)?;
    let activation = AuditRecord::decode_from_slice(body)?;
    assert!(matches!(activation.detail, Some(Detail::CredentialActivation(fact)) if fact.binding.spiffe_id == LOCAL_WORKLOAD && fact.registration_revision == 1));
    shutdown.cancel();
    task.await?;
    workload_api.stop().await;
    Ok(())
}
