use std::error::Error;
use std::io;
use std::num::NonZeroU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use trogon_audit::{AuditName, WriterId, WriterIdentity};
use trogon_egress_proxy::audit_config::AuditConfig;
use trogon_egress_proxy::config::{Config, PolicyInput, TlsInput};
use trogon_egress_proxy::limits::Limits;
use trogon_egress_proxy::live_policy::KvPolicyConfig;
use trogon_egress_proxy::trust_domain::TrustDomain;
use trogon_egress_proxy::workload_identity_config::WorkloadIdentityConfig;
use trogon_egress_proxy::{ReadinessStatus, ScopedSnapshotKey, start};
use trogon_nats::test_support::TlsJetStreamCluster;
use trogon_nats::{DottedNatsToken, NatsToken};
use trogon_workload_identity::test_support::{TestCa, WorkloadApiFixture};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

type TestResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const WAIT: Duration = Duration::from_secs(15);
const REGISTRY: &str = "SCOPED_REGISTRATIONS";
const POLICY: &str = "SCOPED_EGRESS_POLICY";
const LOCAL: &str = "spiffe://trogon.example/service/egress";
const REPLIES: &str = "_INBOX.egress.SCOPED_REGISTRATIONS";

async fn exclusive(
    context: &async_nats::jetstream::Context,
    store: &async_nats::jetstream::kv::Store,
    key: &str,
) -> TestResult<()> {
    let mut config = store.stream.get_info().await?.config;
    assert_eq!(config.num_replicas, 3);
    config.subjects = vec![format!("$KV.{}.{}", store.name, key)];
    config.max_messages = 1;
    config.discard = async_nats::jetstream::stream::DiscardPolicy::Old;
    context.update_stream(config).await?;
    Ok(())
}

#[tokio::test]
async fn workload_runtime_uses_scoped_replies_for_policy_registration_and_audit() -> TestResult<()> {
    let reader = NatsToken::new("egress_reader")?;
    let setup = NatsToken::new("egress_setup")?;
    let key = ScopedSnapshotKey::new(&AuditName::new("acme")?, &NatsToken::new("payments")?);
    let setup_prefix = DottedNatsToken::new("_INBOX.fixture.egress_setup.SCOPED_REGISTRATIONS")?;
    let cluster = TlsJetStreamCluster::start(&[(reader.clone(), json!({
        "publish": [
            format!("$JS.API.STREAM.INFO.KV_{REGISTRY}"),
            format!("$JS.API.STREAM.MSG.GET.KV_{REGISTRY}"),
            format!("$JS.API.STREAM.INFO.KV_{POLICY}"),
            format!("$JS.API.STREAM.MSG.GET.KV_{POLICY}"),
            "platform.v1.audit.integrity.egress_proxy.chain_reset",
            "platform.v1.audit.integrity.egress_proxy.readiness_probe",
            "platform.v1.audit.change.payments.credential",
            "platform.v1.audit.decision.payments.allow",
            "platform.v1.audit.decision.payments.deny"
        ],
        "subscribe": [format!("{REPLIES}.>")]
    })), (setup.clone(), json!({
        "publish": ["$JS.API.>", format!("$KV.{REGISTRY}.{}", key.as_str()), format!("$KV.{POLICY}.{}", key.as_str())],
        "subscribe": [format!("{}.>", setup_prefix.as_str())]
    }))]).await;
    let admin = cluster
        .config(&setup)
        .ok_or_else(|| io::Error::other("setup fixture"))?;
    let admin = trogon_nats::connect_once_with_inbox_prefix(&admin, admin.auth.clone(), WAIT, &setup_prefix).await?;
    let context = async_nats::jetstream::new(admin);
    let audit_stream = context
        .create_stream(async_nats::jetstream::stream::Config {
            name: trogon_audit::constants::STREAM_AUDIT.into(),
            subjects: vec!["platform.v1.audit.>".into()],
            num_replicas: 3,
            max_bytes: 1024 * 1024,
            discard: async_nats::jetstream::stream::DiscardPolicy::New,
            ..Default::default()
        })
        .await?;
    let nats = cluster
        .config(&reader)
        .ok_or_else(|| io::Error::other("reader fixture"))?;
    let audit = AuditConfig {
        nats: nats.clone(),
        identity: WriterIdentity::new(
            WriterId::new("egress-proxy:scoped-fixture")?,
            NatsToken::new("egress_proxy")?,
            AuditName::new("acme")?,
            NatsToken::new("payments")?,
        ),
    };
    let ca = TestCa::new();
    let workload_api = WorkloadApiFixture::start(vec![ca.issue(LOCAL, Duration::from_secs(120))]).await;
    let trust_domain = TrustDomain::new("trogon.example")?;
    let workload = WorkloadIdentityConfig::new(
        &workload_api.endpoint().as_uri(),
        LOCAL,
        REGISTRY,
        NonZeroU64::MIN,
        &trust_domain,
        &audit,
    )?;
    let policy = KvPolicyConfig::new(
        POLICY,
        audit.identity.tenant().clone(),
        audit.identity.project().clone(),
        NonZeroU64::MIN,
    )?;
    let registry = context
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: REGISTRY.into(),
            num_replicas: 3,
            max_value_size: 256 * 1024,
            max_bytes: 1024 * 1024,
            max_age: Duration::from_secs(325),
            ..Default::default()
        })
        .await?;
    exclusive(&context, &registry, workload.registration_key()).await?;
    let policies = context
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: POLICY.into(),
            num_replicas: 3,
            max_value_size: 256 * 1024,
            max_bytes: 1024 * 1024,
            max_age: Duration::from_secs(325),
            ..Default::default()
        })
        .await?;
    exclusive(&context, &policies, policy.key()).await?;
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
                bindings: vec![RegistrationBinding {
                    spiffe_id: LOCAL.into(),
                    tenant: "acme".into(),
                    project: "payments".into(),
                    role: EnumValue::from(WorkloadRole::EgressProxy),
                    application_id: None,
                }],
                approved_root_sha256: vec![ca.root_sha256().to_vec()],
            }
            .encode_to_vec()
            .into(),
        )
        .await?;
    let seconds = now.duration_since(UNIX_EPOCH)?.as_secs();
    policies
        .put(
            policy.key(),
            serde_json::to_vec(&json!({
                "tenant": "acme", "project": "payments", "incarnation": 1,
                "generation": 1, "issued_at": seconds, "valid_until": seconds + 60, "policy": ""
            }))?
            .into(),
        )
        .await?;

    let prefix = DottedNatsToken::new(REPLIES)?;
    let control = trogon_nats::connect_once_with_inbox_prefix(&nats, nats.auth.clone(), WAIT, &prefix).await?;
    assert!(
        async_nats::jetstream::new(control)
            .get_key_value(REGISTRY)
            .await
            .is_ok(),
        "the exact scoped SDK connector must read this fixture before runtime is exercised"
    );

    let started = start(Config {
        listen: "127.0.0.1:0".parse()?,
        policy: PolicyInput::NatsKv(policy),
        tls: TlsInput::Workload(workload),
        trust_domain,
        audit,
        non_tls_ports: Vec::new(),
        limits: Limits::default(),
    })
    .await;
    let server = match started {
        Ok(server) => server,
        Err(error) => {
            workload_api.stop().await;
            return Err(error.into());
        }
    };
    let readiness = server.readiness();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    let ready = tokio::time::timeout(WAIT, async {
        loop {
            if readiness.probe().await == ReadinessStatus::Ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    shutdown.cancel();
    task.await?;
    workload_api.stop().await;
    ready?;
    assert!(
        audit_stream
            .get_last_raw_message_by_subject("platform.v1.audit.change.payments.credential")
            .await
            .is_ok()
    );
    assert!(
        audit_stream
            .get_last_raw_message_by_subject("platform.v1.audit.integrity.egress_proxy.readiness_probe")
            .await
            .is_ok()
    );
    Ok(())
}
