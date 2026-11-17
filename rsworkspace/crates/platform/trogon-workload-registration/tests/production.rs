#![allow(clippy::expect_used, clippy::panic)]

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use async_nats::jetstream::Context;
use async_nats::jetstream::message::StreamMessage;
use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};
use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use trogon_audit::{AuditSubject, ChangeTerminal, IntegrityTerminal, record_seal};
use trogon_nats::test_support::TlsJetStreamCluster;
use trogon_nats::{NatsConfig, NatsTls, NatsToken, connect_once};
use trogon_workload_registration::{
    ApprovedRegistration, ProducerConfig, ProducerStatus, ProductionRegistrationProducer, RegistrationError,
    RegistrationRole, Sha256Digest, StreamProfile, approval_signing_bytes, production_emergency_withdrawal,
    provision_streams,
};
use trogonai_proto::platform::audit::v1::{
    AuditClass, AuditRecord, ChangeKind, audit_actor::Kind as ActorKind, audit_record::Detail,
};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

enum Effect {
    Grant,
    Withdrawal,
}

fn role_suffix(role: RegistrationRole) -> &'static str {
    match role {
        RegistrationRole::Provisioner => "provisioner",
        RegistrationRole::Publisher => "publisher",
        RegistrationRole::Revoker => "revoker",
    }
}

fn role_name(config: &ProducerConfig, role: RegistrationRole) -> NatsToken {
    NatsToken::new(format!("{}_{}", role_suffix(role), config.recovery_fence())).expect("fixture role")
}

fn require_verified_tls(config: &NatsConfig) {
    assert!(matches!(
        &config.tls,
        Ok(NatsTls::Enabled {
            ca: Some(_),
            client_cert: Some(_),
        })
    ));
}

struct NativeCore {
    cluster: TlsJetStreamCluster,
    configs: Vec<ProducerConfig>,
    context: Context,
}

impl NativeCore {
    async fn start() -> Self {
        let key = SigningKey::from_bytes(&[7; 32]);
        let configs: Vec<_> = (1..=3)
            .map(|fence| {
                ProducerConfig::new(
                    "acme",
                    "payments",
                    "identity.example.org",
                    NonZeroU64::new(fence).expect("independent recovery fence"),
                    key.verifying_key(),
                )
                .expect("governed producer configuration")
            })
            .collect();
        let mut users = Vec::new();
        for config in &configs {
            let profile = StreamProfile::new(config);
            for role in [
                RegistrationRole::Provisioner,
                RegistrationRole::Publisher,
                RegistrationRole::Revoker,
            ] {
                users.push((
                    role_name(config, role),
                    json!({
                        "publish": profile.publish_allowlist(role),
                        "subscribe": profile.subscribe_allowlist(role),
                    }),
                ));
            }
        }
        let cluster = TlsJetStreamCluster::start(&users).await;
        let administrator = cluster
            .config(&NatsToken::new("fixture-admin").expect("fixture administrator"))
            .expect("administrator TLS credentials");
        require_verified_tls(&administrator);
        let client = connect_once(&administrator, administrator.auth.clone(), Duration::from_secs(5))
            .await
            .expect("verified authenticated administrator connection");
        let context = async_nats::jetstream::new(client);
        context
            .create_stream(Config {
                name: trogon_audit::constants::STREAM_AUDIT.into(),
                subjects: vec![
                    "platform.v1.audit.decision.>".into(),
                    "platform.v1.audit.credential.>".into(),
                    "platform.v1.audit.change.>".into(),
                    "platform.v1.audit.integrity.>".into(),
                ],
                num_replicas: 3,
                storage: StorageType::File,
                retention: RetentionPolicy::Limits,
                discard: DiscardPolicy::New,
                deny_delete: true,
                deny_purge: true,
                duplicate_window: Duration::from_secs(120),
                max_bytes: 16 * 1024 * 1024,
                ..Default::default()
            })
            .await
            .expect("independently provision protected durable audit storage");
        let fixture = Self {
            cluster,
            configs,
            context,
        };
        for config in &fixture.configs {
            provision_streams(&fixture.role(config, RegistrationRole::Provisioner), config)
                .await
                .expect("exact scoped provisioner can create protected streams");
        }
        fixture
    }

    fn role(&self, config: &ProducerConfig, role: RegistrationRole) -> NatsConfig {
        let nats = self
            .cluster
            .config(&role_name(config, role))
            .expect("distinct scoped native role");
        require_verified_tls(&nats);
        nats
    }
}

fn approved(config: &ProducerConfig, revision: u64, issued: SystemTime, effect: Effect) -> ApprovedRegistration {
    let snapshot = RegistrationSnapshot {
        tenant: config.scope().tenant().as_str().into(),
        project: config.scope().project().as_str().into(),
        recovery_fence: config.recovery_fence().get(),
        registration_revision: revision,
        trust_revision: 1,
        issued_at: MessageField::some(Timestamp::from(issued)),
        expires_at: MessageField::some(Timestamp::from(issued + Duration::from_secs(120))),
        bindings: match effect {
            Effect::Withdrawal => vec![],
            Effect::Grant => vec![RegistrationBinding {
                spiffe_id: "spiffe://identity.example.org/service/egress".into(),
                tenant: config.scope().tenant().as_str().into(),
                project: config.scope().project().as_str().into(),
                role: EnumValue::from(WorkloadRole::EgressProxy),
                application_id: None,
            }],
        },
        approved_root_sha256: vec![vec![1; 32]],
    };
    let payload = snapshot.encode_to_vec();
    let signature = SigningKey::from_bytes(&[7; 32])
        .sign(&approval_signing_bytes(config, &payload))
        .to_bytes();
    ApprovedRegistration::verify(config, &payload, &signature).expect("independently signed finite approval")
}

async fn latest_live(context: &Context, profile: &StreamProfile, payload: &[u8]) -> StreamMessage {
    let message = context
        .get_stream(profile.live_config().name)
        .await
        .expect("actual scoped live stream")
        .get_last_raw_message_by_subject(&profile.live_subject())
        .await
        .expect("acknowledged current authority");
    assert_eq!(message.payload.as_ref(), payload);
    message
}

async fn assert_native_audit(
    context: &Context,
    config: &ProducerConfig,
    approval: &ApprovedRegistration,
    role: RegistrationRole,
    source_sequence: u64,
) {
    let profile = StreamProfile::new(config);
    let stream = context
        .get_stream(trogon_audit::constants::STREAM_AUDIT)
        .await
        .expect("independently provisioned native audit stream");
    let subject = AuditSubject::change(config.scope().project().clone(), ChangeTerminal::Policy).render();
    let stored = stream
        .get_last_raw_message_by_subject(&subject)
        .await
        .expect("durable acknowledged policy change");
    let (body, hash) = record_seal::open(&stored.payload).expect("native writer hash seal");
    let hashed = AuditRecord::decode_from_slice(body).expect("exact hashed audit body");
    assert!(hashed.integrity.record_hash.is_none());
    let record = AuditRecord::decode_from_slice(&stored.payload).expect("stored native audit contract");
    assert_eq!(record.class.as_known(), Some(AuditClass::Must));
    assert!(record.attribution.is_unset());
    assert_eq!(record.tenant.value, config.scope().tenant().as_str());
    assert_eq!(record.project.value, config.scope().project().as_str());
    let component = NatsToken::new(format!("{}_{}", profile.live_bucket(), role_suffix(role)))
        .expect("independently expected scoped writer");
    let integrity = record.integrity.as_option().expect("stored chain identity");
    assert_eq!(integrity.writer_id, component.as_str());
    assert_eq!(integrity.record_hash.as_deref(), Some(hash.as_bytes().as_slice()));
    let reset_subject = AuditSubject::integrity(component, IntegrityTerminal::ChainReset).render();
    let reset = stream
        .get_last_raw_message_by_subject(&reset_subject)
        .await
        .expect("this role can publish only its own scoped chain reset");
    record_seal::open(&reset.payload).expect("sealed role-specific chain genesis");
    let reset = AuditRecord::decode_from_slice(&reset.payload).expect("native chain reset contract");
    let chain = integrity.chain_id.as_deref().expect("native writer chain identifier");
    assert_eq!(reset.integrity.chain_id.as_deref(), Some(chain));
    let Some(ActorKind::ApiKey(actor)) = record.actor.kind.as_ref() else {
        panic!("native audit must preserve pinned approval-key attribution");
    };
    assert_eq!(
        actor.api_key_id,
        format!("ed25519_sha256:{}", config.approval_key().fingerprint().to_hex())
    );
    let Some(Detail::Change(change)) = record.detail.as_ref() else {
        panic!("native writer must emit a policy change");
    };
    assert_eq!(change.kind, EnumValue::from(ChangeKind::CHANGE_KIND_POLICY));
    assert_eq!(change.resource_name, profile.live_bucket());
    assert_eq!(
        change.event_id,
        format!("sha256:{}", Sha256Digest::hash(&approval.payload()).to_hex())
    );
    let (source_stream, source_subject, event_type) = if approval.is_withdrawal() {
        (
            profile.live_config().name,
            profile.live_subject(),
            "workload_registration_withdrawal",
        )
    } else {
        (
            profile.control_config().name,
            profile.control_subject(),
            "workload_registration_grant_intent",
        )
    };
    assert_eq!(change.event_type, event_type);
    assert_eq!(change.source_stream, source_stream);
    assert_eq!(change.stream_sequence, source_sequence);
    let source = context
        .get_stream(source_stream)
        .await
        .expect("native audited source stream")
        .get_last_raw_message_by_subject(&source_subject)
        .await
        .expect("acknowledged native source evidence");
    assert_eq!(source.sequence, source_sequence);
    assert_eq!(source.payload.as_ref(), approval.payload().as_ref());
}

#[tokio::test]
async fn production_core_requires_scoped_tls_durable_audit_and_finite_independent_authority() {
    let fixture = NativeCore::start().await;
    let config = &fixture.configs[0];
    let profile = StreamProfile::new(config);
    let publisher = fixture.role(config, RegistrationRole::Publisher);
    let revoker = fixture.role(config, RegistrationRole::Revoker);
    let issued = SystemTime::now() - Duration::from_secs(5);
    let initial = approved(config, 1, issued, Effect::Grant);
    let initial_deadline = initial.lease().deadline();
    let mut producer = ProductionRegistrationProducer::start(config.clone(), publisher, initial.clone())
        .await
        .expect("scoped native producer can claim only fresh governed authority");
    assert_eq!(producer.status(), ProducerStatus::Active);
    assert_eq!(producer.current_deadline(), initial_deadline);
    assert_eq!(
        latest_live(&fixture.context, &profile, &initial.payload())
            .await
            .sequence,
        1
    );
    assert_native_audit(&fixture.context, config, &initial, RegistrationRole::Publisher, 1).await;

    let renewal = approved(config, 2, issued + Duration::from_secs(1), Effect::Grant);
    let renewed_deadline = renewal.lease().deadline();
    assert!(renewed_deadline > initial_deadline);
    let receipt = producer
        .apply(renewal.clone())
        .await
        .expect("independent signed renewal");
    assert_eq!(producer.current_deadline(), renewed_deadline);
    assert_eq!(receipt.control_sequence().get(), 2);
    assert_eq!(receipt.live_sequence().get(), 2);
    assert_eq!(
        latest_live(&fixture.context, &profile, &renewal.payload())
            .await
            .sequence,
        2
    );
    assert_native_audit(
        &fixture.context,
        config,
        &renewal,
        RegistrationRole::Publisher,
        receipt.control_sequence().get(),
    )
    .await;

    let denial = approved(config, 3, issued + Duration::from_secs(2), Effect::Withdrawal);
    let receipt = production_emergency_withdrawal(&revoker, config, denial.clone())
        .await
        .expect("independent scoped revoker publishes durable native withdrawal");
    assert_eq!(receipt.control_sequence().get(), 3);
    assert_eq!(receipt.live_sequence().get(), 3);
    let live = latest_live(&fixture.context, &profile, &denial.payload()).await;
    assert_eq!(live.sequence, receipt.live_sequence().get());
    assert_native_audit(
        &fixture.context,
        config,
        &denial,
        RegistrationRole::Revoker,
        receipt.live_sequence().get(),
    )
    .await;
    assert!(matches!(
        producer
            .apply(approved(config, 4, issued + Duration::from_secs(3), Effect::Grant))
            .await,
        Err(RegistrationError::HeadChanged)
    ));
    assert_eq!(producer.status(), ProducerStatus::Poisoned);
    latest_live(&fixture.context, &profile, &denial.payload()).await;

    let config = &fixture.configs[1];
    let profile = StreamProfile::new(config);
    let publisher = fixture.role(config, RegistrationRole::Publisher);
    let revoker = fixture.role(config, RegistrationRole::Revoker);
    let issued = SystemTime::now() - Duration::from_secs(5);
    let initial = approved(config, 1, issued, Effect::Grant);
    let initial_deadline = initial.lease().deadline();
    let mut producer = ProductionRegistrationProducer::start(config.clone(), publisher, initial.clone())
        .await
        .expect("second independently fenced native producer");
    assert_eq!(producer.current_deadline(), initial_deadline);
    assert_native_audit(&fixture.context, config, &initial, RegistrationRole::Publisher, 1).await;
    fixture
        .context
        .delete_stream(trogon_audit::constants::STREAM_AUDIT)
        .await
        .expect("actual durable audit outage");
    assert!(matches!(
        producer
            .apply(approved(config, 2, issued + Duration::from_secs(1), Effect::Grant))
            .await,
        Err(RegistrationError::Audit(_))
    ));
    assert_eq!(producer.status(), ProducerStatus::Poisoned);
    assert_eq!(producer.current_deadline(), initial_deadline);
    latest_live(&fixture.context, &profile, &initial.payload()).await;
    let denial = approved(config, 3, issued + Duration::from_secs(2), Effect::Withdrawal);
    let live_sequence = match production_emergency_withdrawal(&revoker, config, denial.clone()).await {
        Err(RegistrationError::WithdrawalIncomplete {
            live_sequence,
            control: None,
            audit: Some(_),
        }) => live_sequence,
        other => panic!("native live denial must retain its acknowledged receipt during audit outage: {other:?}"),
    };
    let live = latest_live(&fixture.context, &profile, &denial.payload()).await;
    assert_eq!(live.sequence, live_sequence.get());
    assert_eq!(live_sequence.get(), 2);

    let config = &fixture.configs[2];
    let profile = StreamProfile::new(config);
    let issued = SystemTime::now() - Duration::from_secs(1);
    assert!(matches!(
        ProductionRegistrationProducer::start(
            config.clone(),
            fixture.role(config, RegistrationRole::Publisher),
            approved(config, 1, issued, Effect::Grant),
        )
        .await,
        Err(RegistrationError::Audit(_))
    ));
    let live = fixture
        .context
        .get_stream(profile.live_config().name)
        .await
        .expect("third scoped live stream");
    assert_eq!(live.cached_info().state.messages, 0);
    assert_eq!(live.cached_info().state.last_sequence, 0);
    let control = fixture
        .context
        .get_stream(profile.control_config().name)
        .await
        .expect("third durable control stream");
    assert_eq!(control.cached_info().state.last_sequence, 1);
}
