#![allow(clippy::expect_used, clippy::panic)]

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use trogon_audit::{AuditName, AuditWriter, MemorySink, SinkError, Stamper, WriterConfig, WriterId, WriterIdentity};
use trogon_nats::test_support::TlsJetStreamCluster;
use trogon_nats::{NatsConfig, NatsToken, connect_once};
use trogon_std::{SystemClock, UuidV7Generator};
use trogon_workload_registration::{
    ApprovedRegistration, ProducerConfig, ProducerStatus, RegistrationError, RegistrationProducer, RegistrationRole,
    StreamProfile, approval_signing_bytes, emergency_withdrawal, provision_streams,
};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

fn approved(config: &ProducerConfig, revision: u64, issued: SystemTime, withdrawn: bool) -> ApprovedRegistration {
    let snapshot = RegistrationSnapshot {
        tenant: "acme".into(),
        project: "payments".into(),
        recovery_fence: config.recovery_fence().get(),
        registration_revision: revision,
        trust_revision: 1,
        issued_at: MessageField::some(Timestamp::from(issued)),
        expires_at: MessageField::some(Timestamp::from(issued + Duration::from_secs(120))),
        bindings: if withdrawn {
            vec![]
        } else {
            vec![RegistrationBinding {
                spiffe_id: "spiffe://identity.example.org/service/egress".into(),
                tenant: "acme".into(),
                project: "payments".into(),
                role: EnumValue::from(WorkloadRole::EgressProxy),
                application_id: None,
            }]
        },
        approved_root_sha256: vec![vec![1; 32]],
    };
    let key = SigningKey::from_bytes(&[7; 32]);
    let payload = snapshot.encode_to_vec();
    let signature = key.sign(&approval_signing_bytes(config, &payload)).to_bytes();
    ApprovedRegistration::verify(config, &payload, &signature).expect("independent fresh signed approval")
}

fn writer(sink: MemorySink) -> AuditWriter<MemorySink> {
    let identity = WriterIdentity::new(
        WriterId::new("registration-test").expect("writer"),
        NatsToken::new("workload_registration").expect("component"),
        AuditName::new("acme").expect("tenant"),
        NatsToken::new("payments").expect("project"),
    );
    AuditWriter::new(
        sink,
        identity,
        WriterConfig::default(),
        &Stamper::new(SystemClock, UuidV7Generator),
    )
}

async fn latest(nats: &NatsConfig, profile: &StreamProfile) -> async_nats::jetstream::message::StreamMessage {
    let client = connect_once(nats, nats.auth.clone(), Duration::from_secs(5))
        .await
        .expect("fresh verified reader");
    async_nats::jetstream::new(client)
        .get_stream(profile.live_config().name)
        .await
        .expect("scoped live stream")
        .get_last_raw_message_by_subject(&profile.live_subject())
        .await
        .expect("current scoped authority")
}

#[tokio::test]
async fn acknowledged_emergency_denial_fences_a_grant_waiting_for_audit_and_same_fence_reclaim() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let config = ProducerConfig::new(
        "acme",
        "payments",
        "identity.example.org",
        NonZeroU64::MIN,
        key.verifying_key(),
    )
    .expect("configuration");
    let profile = StreamProfile::new(&config);
    let publisher = NatsToken::new("publisher").expect("publisher");
    let revoker = NatsToken::new("revoker").expect("revoker");
    let users = [
        (publisher.clone(), RegistrationRole::Publisher),
        (revoker.clone(), RegistrationRole::Revoker),
    ]
    .map(|(name, role)| {
        (
            name,
            json!({"publish": profile.publish_allowlist(role), "subscribe": profile.subscribe_allowlist(role)}),
        )
    });
    let cluster = TlsJetStreamCluster::start(&users).await;
    let nats = cluster.config(&publisher).expect("publisher credential");
    let emergency = cluster.config(&revoker).expect("revoker credential");
    let admin = cluster
        .config(&NatsToken::new("fixture-admin").expect("administrator"))
        .expect("fixture credential");
    provision_streams(&admin, &config)
        .await
        .expect("protected production stream provisioning");
    let sink = MemorySink::new();
    let issued = SystemTime::now() - Duration::from_secs(5);
    let mut producer = RegistrationProducer::claim(
        config.clone(),
        nats.clone(),
        approved(&config, 1, issued, false),
        writer(sink.clone()),
    )
    .await
    .expect("one protected writer claims fresh scope");
    assert_eq!(producer.status(), ProducerStatus::Active);
    let initial = latest(&admin, &profile).await;
    assert_eq!(
        RegistrationSnapshot::decode_from_slice(&initial.payload)
            .expect("runtime wire contract")
            .registration_revision,
        1
    );
    let attempts = sink.attempts();
    sink.hold();
    let result = {
        let update = producer.apply(approved(&config, 2, issued + Duration::from_secs(1), false));
        tokio::pin!(update);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    result = &mut update => panic!("grant bypassed required audit wait: {result:?}"),
                    () = tokio::time::sleep(Duration::from_millis(10)) => if sink.attempts() > attempts { break; },
                }
            }
        })
        .await
        .expect("grant reaches actual held audit sink");
        let denial = emergency_withdrawal(
            &emergency,
            &config,
            approved(&config, 3, issued + Duration::from_secs(2), true),
            &writer(MemorySink::new()),
        )
        .await
        .expect("independent denial acknowledged before grant audit is released");
        let denied = latest(&admin, &profile).await;
        let restarted_reader =
            RegistrationSnapshot::decode_from_slice(&denied.payload).expect("new reader receives complete denial");
        assert!(restarted_reader.bindings.is_empty());
        assert_eq!(restarted_reader.registration_revision, 3);
        assert_eq!(denied.sequence, denial.live_sequence().get());
        sink.release();
        tokio::time::timeout(Duration::from_secs(5), &mut update)
            .await
            .expect("old grant terminates after audit release")
    };
    assert!(
        result.is_err(),
        "superseded grant must fail its broker sequence comparison"
    );
    assert_eq!(producer.status(), ProducerStatus::Poisoned);
    let final_message = latest(&admin, &profile).await;
    assert_eq!(
        RegistrationSnapshot::decode_from_slice(&final_message.payload)
            .expect("final authority")
            .registration_revision,
        3
    );
    drop(producer);
    assert!(
        RegistrationProducer::claim(
            config.clone(),
            nats,
            approved(&config, 4, issued + Duration::from_secs(3), false),
            writer(MemorySink::new())
        )
        .await
        .is_err(),
        "existing durable ownership cannot be reclaimed or renewed from stale state"
    );
}

fn resign(config: &ProducerConfig, snapshot: &RegistrationSnapshot) -> ApprovedRegistration {
    let payload = snapshot.encode_to_vec();
    let signature = SigningKey::from_bytes(&[7; 32])
        .sign(&approval_signing_bytes(config, &payload))
        .to_bytes();
    ApprovedRegistration::verify(config, &payload, &signature).expect("fresh independently signed snapshot")
}

#[tokio::test]
async fn protected_lifecycle_refuses_unaudited_grants_cancelled_owners_and_malformed_streams() {
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
    let users = [(publisher.clone(), RegistrationRole::Publisher), (revoker.clone(), RegistrationRole::Revoker)]
        .map(|(name, role)| (name, json!({
            "publish": configs.iter().flat_map(|config| StreamProfile::new(config).publish_allowlist(role)).collect::<Vec<_>>(),
            "subscribe": configs.iter().flat_map(|config| StreamProfile::new(config).subscribe_allowlist(role)).collect::<Vec<_>>(),
        })));
    let cluster = TlsJetStreamCluster::start(&users).await;
    let admin = cluster
        .config(&NatsToken::new("fixture-admin").expect("admin"))
        .expect("admin config");
    let nats = cluster.config(&publisher).expect("publisher config");
    let emergency = cluster.config(&revoker).expect("revoker config");
    for config in &configs[..3] {
        provision_streams(&admin, config).await.expect("protected streams");
    }

    let config = &configs[0];
    let profile = StreamProfile::new(config);
    let issued = SystemTime::now() - Duration::from_secs(10);
    let sink = MemorySink::new();
    let mut producer = RegistrationProducer::claim(
        config.clone(),
        nats.clone(),
        approved(config, 1, issued, false),
        writer(sink.clone()),
    )
    .await
    .expect("fresh exclusive writer");
    let mut changed =
        RegistrationSnapshot::decode_from_slice(&approved(config, 2, issued + Duration::from_secs(1), false).payload())
            .expect("snapshot");
    changed.bindings[0].spiffe_id = "spiffe://identity.example.org/service/replacement".into();
    assert!(matches!(
        producer.apply(resign(config, &changed)).await,
        Err(RegistrationError::WithdrawalRequired)
    ));
    assert_eq!(producer.status(), ProducerStatus::Active);
    producer
        .apply(approved(config, 2, issued + Duration::from_secs(1), false))
        .await
        .expect("unchanged sets renew with a fresh approval");
    producer
        .apply(approved(config, 3, issued + Duration::from_secs(2), true))
        .await
        .expect("signed whole-scope denial barrier");
    let mut stale_preapproval = changed.clone();
    stale_preapproval.registration_revision = 100;
    assert!(matches!(
        producer.apply(resign(config, &stale_preapproval)).await,
        Err(RegistrationError::Revision)
    ));
    changed.registration_revision = 4;
    changed.issued_at = MessageField::some(Timestamp::from(issued + Duration::from_secs(3)));
    changed.trust_revision = 2;
    changed.approved_root_sha256 = vec![vec![2; 32]];
    producer
        .apply(resign(config, &changed))
        .await
        .expect("independently newer grant follows acknowledged denial");

    sink.fail_all(SinkError::Unreachable);
    changed.registration_revision = 5;
    changed.issued_at = MessageField::some(Timestamp::from(issued + Duration::from_secs(4)));
    assert!(matches!(
        producer.apply(resign(config, &changed)).await,
        Err(RegistrationError::Audit(_))
    ));
    assert_eq!(producer.status(), ProducerStatus::Poisoned);
    assert_eq!(
        RegistrationSnapshot::decode_from_slice(&latest(&admin, &profile).await.payload)
            .expect("live snapshot")
            .registration_revision,
        4
    );
    changed.registration_revision = 6;
    changed.issued_at = MessageField::some(Timestamp::from(issued + Duration::from_secs(5)));
    changed.bindings.clear();
    let denial = resign(config, &changed);
    let failing_denial_sink = MemorySink::new();
    failing_denial_sink.fail_all(SinkError::Unreachable);
    let outcome = emergency_withdrawal(&emergency, config, denial, &writer(failing_denial_sink.clone())).await;
    let live_sequence = match outcome {
        Err(RegistrationError::WithdrawalIncomplete {
            live_sequence,
            control: None,
            audit: Some(_),
        }) => live_sequence,
        other => panic!("live-first denial must retain its acknowledged partial receipt: {other:?}"),
    };
    let live = latest(&admin, &profile).await;
    let denied = RegistrationSnapshot::decode_from_slice(&live.payload).expect("live denial");
    assert_eq!(denied.registration_revision, 6);
    assert!(denied.bindings.is_empty());
    assert_eq!(live.sequence, live_sequence.get());
    assert!(failing_denial_sink.attempts() > 0, "audit attempted after live denial");
    assert!(
        RegistrationProducer::claim(
            config.clone(),
            nats.clone(),
            approved(config, 7, issued + Duration::from_secs(6), false),
            writer(MemorySink::new())
        )
        .await
        .is_err()
    );

    let config = &configs[1];
    let profile = StreamProfile::new(config);
    let sink = MemorySink::new();
    let mut producer = RegistrationProducer::claim(
        config.clone(),
        nats.clone(),
        approved(config, 1, issued, false),
        writer(sink.clone()),
    )
    .await
    .expect("new independently provisioned epoch");
    let attempts = sink.attempts();
    sink.hold();
    {
        let update = producer.apply(approved(config, 2, issued + Duration::from_secs(1), false));
        tokio::pin!(update);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    result = &mut update => panic!("update bypassed held required audit: {result:?}"),
                    () = tokio::time::sleep(Duration::from_millis(10)) => if sink.attempts()>attempts { break; },
                }
            }
        })
        .await
        .expect("update reaches audit before cancellation");
    }
    sink.release();
    assert_eq!(producer.status(), ProducerStatus::Poisoned);
    assert!(matches!(
        producer
            .apply(approved(config, 3, issued + Duration::from_secs(2), false))
            .await,
        Err(RegistrationError::Poisoned)
    ));
    assert_eq!(
        RegistrationSnapshot::decode_from_slice(&latest(&admin, &profile).await.payload)
            .expect("live snapshot")
            .registration_revision,
        1
    );
    assert!(
        RegistrationProducer::claim(
            config.clone(),
            nats.clone(),
            approved(config, 3, issued + Duration::from_secs(2), false),
            writer(MemorySink::new())
        )
        .await
        .is_err()
    );

    let config = &configs[2];
    let profile = StreamProfile::new(config);
    let sink = MemorySink::new();
    let mut producer = RegistrationProducer::claim(
        config.clone(),
        nats.clone(),
        approved(config, 1, issued, false),
        writer(sink.clone()),
    )
    .await
    .expect("expiry test epoch");
    let mut expiring =
        RegistrationSnapshot::decode_from_slice(&approved(config, 2, issued + Duration::from_secs(1), false).payload())
            .expect("expiring renewal");
    expiring.expires_at = MessageField::some(Timestamp::from(SystemTime::now() + Duration::from_secs(1)));
    let expiring = resign(config, &expiring);
    sink.hold();
    let outcome = tokio::time::timeout(Duration::from_secs(3), producer.apply(expiring))
        .await
        .expect("original signed expiry bounds required audit");
    sink.release();
    assert!(matches!(outcome, Err(RegistrationError::Timeout)));
    assert_eq!(producer.status(), ProducerStatus::Poisoned);
    assert_eq!(
        RegistrationSnapshot::decode_from_slice(&latest(&admin, &profile).await.payload)
            .expect("unrenewed live snapshot")
            .registration_revision,
        1
    );

    let config = &configs[3];
    let profile = StreamProfile::new(config);
    let client = connect_once(&admin, admin.auth.clone(), Duration::from_secs(5))
        .await
        .expect("administrator");
    let context = async_nats::jetstream::new(client);
    let mut malformed = profile.control_config();
    malformed.max_age = Duration::from_secs(300);
    context
        .create_stream(malformed)
        .await
        .expect("misconfigured durable control fixture");
    assert!(matches!(
        provision_streams(&admin, config).await,
        Err(RegistrationError::StreamProfile)
    ));
    context
        .create_stream(profile.live_config())
        .await
        .expect("live fixture");
    assert!(matches!(
        RegistrationProducer::claim(
            config.clone(),
            nats,
            approved(config, 1, issued, false),
            writer(MemorySink::new())
        )
        .await,
        Err(RegistrationError::StreamProfile)
    ));
}
