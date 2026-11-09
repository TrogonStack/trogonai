#![allow(clippy::expect_used)]

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use trogon_audit::{AuditName, AuditWriter, MemorySink, Stamper, WriterConfig, WriterId, WriterIdentity};
use trogon_nats::test_support::TlsJetStreamCluster;
use trogon_nats::{NatsToken, connect_once, connect_once_with_inbox_prefix};
use trogon_std::{SystemClock, UuidV7Generator};
use trogon_workload_identity::constants::{MAX_BINDINGS, MAX_ROOTS};
use trogon_workload_registration::{
    ApprovedRegistration, MAX_SNAPSHOT_BYTES, ProducerConfig, ProducerStatus, RegistrationError, RegistrationProducer,
    RegistrationRole, StreamProfile, approval_signing_bytes, emergency_withdrawal, provision_streams,
};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

#[path = "recovery/head_failure.rs"]
mod head_failure;
use head_failure::HeadFailure;

fn document(config: &ProducerConfig, issued: SystemTime, revision: u64) -> RegistrationSnapshot {
    RegistrationSnapshot {
        tenant: "acme".into(),
        project: "payments".into(),
        recovery_fence: config.recovery_fence().get(),
        registration_revision: revision,
        trust_revision: 1,
        issued_at: MessageField::some(Timestamp::from(issued)),
        expires_at: MessageField::some(Timestamp::from(issued + Duration::from_secs(120))),
        bindings: vec![RegistrationBinding {
            spiffe_id: "spiffe://identity.example.org/service/egress".into(),
            tenant: "acme".into(),
            project: "payments".into(),
            role: EnumValue::from(WorkloadRole::EgressProxy),
            application_id: None,
        }],
        approved_root_sha256: vec![vec![1; 32]],
    }
}

fn approved(config: &ProducerConfig, document: &RegistrationSnapshot) -> ApprovedRegistration {
    let payload = document.encode_to_vec();
    let signature = SigningKey::from_bytes(&[7; 32])
        .sign(&approval_signing_bytes(config, &payload))
        .to_bytes();
    ApprovedRegistration::verify(config, &payload, &signature).expect("independently signed approval")
}

fn audit() -> AuditWriter<MemorySink> {
    AuditWriter::new(
        MemorySink::new(),
        WriterIdentity::new(
            WriterId::new("registration-recovery").expect("writer"),
            NatsToken::new("registration").expect("component"),
            AuditName::new("acme").expect("tenant"),
            NatsToken::new("payments").expect("project"),
        ),
        WriterConfig::default(),
        &Stamper::new(SystemClock, UuidV7Generator),
    )
}

#[tokio::test]
async fn malformed_durable_heads_refuse_and_independent_denials_and_incarnations_fence_old_owners() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let configs: Vec<_> = (1..=4)
        .map(|fence| {
            ProducerConfig::new(
                "acme",
                "payments",
                "identity.example.org",
                NonZeroU64::new(fence).expect("fence"),
                key.verifying_key(),
            )
            .expect("scope")
        })
        .collect();
    let publisher = NatsToken::new("publisher").expect("publisher");
    let revoker = NatsToken::new("revoker").expect("revoker");
    let users=[(publisher.clone(),RegistrationRole::Publisher),(revoker.clone(),RegistrationRole::Revoker)].map(|(name,role)|(name,json!({
        "publish":configs.iter().flat_map(|config|StreamProfile::new(config).publish_allowlist(role)).collect::<Vec<_>>(),
        "subscribe":configs.iter().flat_map(|config|StreamProfile::new(config).subscribe_allowlist(role)).collect::<Vec<_>>(),
    })));
    let cluster = TlsJetStreamCluster::start(&users).await;
    let admin = cluster
        .config(&NatsToken::new("fixture-admin").expect("admin"))
        .expect("admin config");
    let nats = cluster.config(&publisher).expect("publisher config");
    let emergency = cluster.config(&revoker).expect("revoker config");
    for config in &configs {
        provision_streams(&admin, config).await.expect("protected streams");
    }
    let issued = SystemTime::now() - Duration::from_secs(20);
    let config = &configs[0];
    let profile = StreamProfile::new(config);
    let writer = connect_once_with_inbox_prefix(
        &nats,
        nats.auth.clone(),
        Duration::from_secs(5),
        &profile.inbox_prefix(RegistrationRole::Publisher).expect("scoped inbox"),
    )
    .await
    .expect("scoped trusted raw publisher");
    let writer = async_nats::jetstream::new(writer);
    let base = document(config, issued, 1);
    let mut denial = document(config, issued + Duration::from_secs(2), 2);
    denial.bindings.clear();
    let mut vectors = Vec::new();
    let mut invalid = base.clone();
    invalid.tenant = "other".into();
    vectors.push((invalid, HeadFailure::Scope));
    invalid = base.clone();
    invalid.recovery_fence = 2;
    vectors.push((invalid, HeadFailure::Scope));
    invalid = base.clone();
    invalid.issued_at = MessageField::some(Timestamp {
        seconds: 0,
        nanos: 0,
        ..Timestamp::default()
    });
    vectors.push((invalid, HeadFailure::Lease));
    invalid = base.clone();
    invalid.issued_at = MessageField::some(Timestamp {
        seconds: 1,
        nanos: -1,
        ..Timestamp::default()
    });
    vectors.push((invalid, HeadFailure::Lease));
    invalid = base.clone();
    invalid.expires_at = MessageField::some(Timestamp::from(issued + Duration::from_secs(331)));
    vectors.push((invalid, HeadFailure::Bounds));
    invalid = base.clone();
    invalid.bindings = vec![base.bindings[0].clone(); MAX_BINDINGS + 1];
    vectors.push((invalid, HeadFailure::Bounds));
    invalid = base.clone();
    invalid.approved_root_sha256 = vec![vec![1; 32]; MAX_ROOTS + 1];
    vectors.push((invalid, HeadFailure::Bounds));
    invalid = base.clone();
    invalid.bindings[0].role = EnumValue::from(WorkloadRole::Unspecified);
    vectors.push((invalid, HeadFailure::Scope));
    invalid = base.clone();
    invalid.bindings[0].role = EnumValue::from(WorkloadRole::Agent);
    vectors.push((invalid, HeadFailure::ApplicationIdentity));
    invalid = base.clone();
    invalid.bindings[0].role = EnumValue::from(WorkloadRole::Agent);
    invalid.bindings[0].application_id = Some("agent-alias".into());
    vectors.push((invalid, HeadFailure::ApplicationIdentity));
    invalid = base.clone();
    invalid.bindings[0].tenant = "other".into();
    vectors.push((invalid, HeadFailure::Scope));
    invalid = base.clone();
    invalid.bindings.push(base.bindings[0].clone());
    vectors.push((invalid, HeadFailure::Scope));
    invalid = base.clone();
    invalid.bindings.push(RegistrationBinding {
        spiffe_id: "spiffe://identity.example.org/service/alpha".into(),
        ..base.bindings[0].clone()
    });
    vectors.push((invalid, HeadFailure::Scope));
    invalid = base.clone();
    invalid.approved_root_sha256 = vec![vec![1; 32], vec![1; 32]];
    vectors.push((invalid, HeadFailure::Bounds));
    invalid = base.clone();
    invalid.approved_root_sha256 = vec![vec![2; 32], vec![1; 32]];
    vectors.push((invalid, HeadFailure::Bounds));
    invalid = base.clone();
    invalid.issued_at = MessageField::some(Timestamp::from(issued + Duration::from_secs(3)));
    vectors.push((invalid, HeadFailure::Revision));
    invalid = base.clone();
    invalid.approved_root_sha256 = vec![vec![2; 32]];
    vectors.push((invalid, HeadFailure::Revision));
    for (invalid, expected) in vectors {
        writer
            .publish(profile.control_subject(), invalid.encode_to_vec().into())
            .await
            .expect("enqueue malformed trusted control fixture")
            .await
            .expect("durable malformed fixture");
        expected.assert_refused(emergency_withdrawal(&emergency, config, approved(config, &denial), &audit()).await);
        assert_eq!(
            writer
                .get_stream(profile.live_config().name)
                .await
                .expect("live stream")
                .get_info()
                .await
                .expect("live info")
                .state
                .last_sequence,
            0
        );
    }
    let mut noncanonical = base.encode_to_vec();
    noncanonical.extend_from_slice(&[0x20, 0x01]);
    writer
        .publish(profile.control_subject(), noncanonical.into())
        .await
        .expect("enqueue duplicate field fixture")
        .await
        .expect("persist duplicate field fixture");
    HeadFailure::Noncanonical
        .assert_refused(emergency_withdrawal(&emergency, config, approved(config, &denial), &audit()).await);
    let before = writer
        .get_stream(profile.control_config().name)
        .await
        .expect("control stream")
        .get_info()
        .await
        .expect("control info")
        .state
        .last_sequence;
    assert!(
        writer
            .publish(profile.control_subject(), Bytes::from(vec![0; MAX_SNAPSHOT_BYTES + 1]))
            .await
            .expect("bounded native enqueue")
            .await
            .is_err()
    );
    assert_eq!(
        writer
            .get_stream(profile.control_config().name)
            .await
            .expect("control stream")
            .get_info()
            .await
            .expect("control info")
            .state
            .last_sequence,
        before
    );

    let application = format!(
        "did:key:z{}",
        bs58::encode(
            [
                &[0xed, 0x01][..],
                SigningKey::from_bytes(&[9; 32]).verifying_key().as_bytes()
            ]
            .concat()
        )
        .into_string()
    );
    let mut valid = base;
    valid.bindings[0].role = EnumValue::from(WorkloadRole::Agent);
    valid.bindings[0].spiffe_id = "spiffe://identity.example.org/runtime/agent".into();
    valid.bindings[0].application_id = Some(application);
    valid.bindings.push(RegistrationBinding {
        spiffe_id: "spiffe://identity.example.org/service/supervisor".into(),
        tenant: "acme".into(),
        project: "payments".into(),
        role: EnumValue::from(WorkloadRole::Supervisor),
        application_id: None,
    });
    writer
        .publish(profile.control_subject(), valid.encode_to_vec().into())
        .await
        .expect("enqueue valid complete head")
        .await
        .expect("persist valid complete head");
    let receipt = emergency_withdrawal(&emergency, config, approved(config, &denial), &audit())
        .await
        .expect("governed withdrawal from valid Agent and Supervisor head");
    let control = writer
        .get_key_value(profile.control_bucket())
        .await
        .expect("actual native control bucket")
        .entry(config.scope().key())
        .await
        .expect("control read")
        .expect("current durable denial");
    assert_eq!(control.revision, receipt.control_sequence().get());
    assert!(
        RegistrationSnapshot::decode_from_slice(&control.value)
            .expect("durable denial wire")
            .bindings
            .is_empty()
    );
    let live = writer
        .get_key_value(profile.live_bucket())
        .await
        .expect("actual native live bucket")
        .entry(config.scope().key())
        .await
        .expect("live read")
        .expect("current live denial");
    assert_eq!(live.revision, receipt.live_sequence().get());

    let config = &configs[1];
    let mut denial = document(config, issued, 1);
    denial.bindings.clear();
    let mut owner = RegistrationProducer::claim(config.clone(), nats.clone(), approved(config, &denial), audit())
        .await
        .expect("fresh epoch can start at a whole-scope denial");
    owner
        .apply(approved(config, &document(config, issued + Duration::from_secs(1), 2)))
        .await
        .expect("fresh grant after initial denial");
    denial.registration_revision = 3;
    denial.issued_at = MessageField::some(Timestamp::from(issued + Duration::from_secs(2)));
    emergency_withdrawal(&emergency, config, approved(config, &denial), &audit())
        .await
        .expect("independent denial advances the live head");
    assert!(matches!(
        owner
            .apply(approved(config, &document(config, issued + Duration::from_secs(3), 4)))
            .await,
        Err(RegistrationError::HeadChanged)
    ));
    assert_eq!(owner.status(), ProducerStatus::Poisoned);

    let config = &configs[2];
    let profile = StreamProfile::new(config);
    let mut owner = RegistrationProducer::claim(
        config.clone(),
        nats,
        approved(config, &document(config, issued, 1)),
        audit(),
    )
    .await
    .expect("fresh epoch writer");
    let admin_client = connect_once(&admin, admin.auth.clone(), Duration::from_secs(5))
        .await
        .expect("administrator");
    let admin = async_nats::jetstream::new(admin_client);
    admin
        .delete_stream(profile.live_config().name)
        .await
        .expect("explicit stream replacement fault");
    admin
        .create_stream(profile.live_config())
        .await
        .expect("same-profile replacement stream");
    assert!(matches!(
        owner
            .apply(approved(config, &document(config, issued + Duration::from_secs(1), 2)))
            .await,
        Err(RegistrationError::HeadChanged)
    ));
    assert_eq!(owner.status(), ProducerStatus::Poisoned);
    assert_eq!(
        admin
            .get_stream(profile.live_config().name)
            .await
            .expect("replacement stream")
            .get_info()
            .await
            .expect("replacement info")
            .state
            .last_sequence,
        0
    );

    let config = &configs[3];
    let profile = StreamProfile::new(config);
    let expired = document(config, SystemTime::now() - Duration::from_secs(600), 9);
    let payload = expired.encode_to_vec();
    let signature = key.sign(&approval_signing_bytes(config, &payload)).to_bytes();
    assert!(matches!(
        ApprovedRegistration::verify(config, &payload, &signature),
        Err(RegistrationError::Lease)
    ));
    writer
        .publish(profile.control_subject(), payload.into())
        .await
        .expect("enqueue expired durable head")
        .await
        .expect("persist expired durable head");
    let mut denial = document(config, SystemTime::now() - Duration::from_secs(1), 9);
    denial.bindings.clear();
    assert!(matches!(
        emergency_withdrawal(&emergency, config, approved(config, &denial), &audit()).await,
        Err(RegistrationError::Revision)
    ));
    denial.registration_revision = 10;
    emergency_withdrawal(&emergency, config, approved(config, &denial), &audit())
        .await
        .expect("expired historical head still fences a strictly newer withdrawal");
    let live = writer
        .get_key_value(profile.live_bucket())
        .await
        .expect("native live bucket")
        .entry(config.scope().key())
        .await
        .expect("live read")
        .expect("durable newer withdrawal");
    let live = RegistrationSnapshot::decode_from_slice(&live.value).expect("native withdrawal wire");
    assert_eq!(live.registration_revision, 10);
    assert!(live.bindings.is_empty());
}
