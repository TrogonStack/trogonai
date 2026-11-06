#![allow(clippy::expect_used, clippy::panic)]

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use trogon_audit::{AuditName, AuditWriter, MemorySink, Stamper, WriterConfig, WriterId, WriterIdentity};
use trogon_nats::test_support::TlsJetStreamCluster;
use trogon_nats::{NatsConfig, NatsToken, connect_once};
use trogon_std::{SystemClock, UuidV7Generator};
use trogon_workload_registration::{
    ApprovedRegistration, ProducerConfig, ProducerStatus, RegistrationProducer, RegistrationRole, SIGNING_CONTEXT,
    StreamProfile, emergency_withdrawal, provision_streams,
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
    let signature = key.sign(&[SIGNING_CONTEXT, &payload].concat()).to_bytes();
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
