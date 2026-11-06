#![allow(clippy::expect_used, clippy::panic)]

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use trogon_audit::{AuditName, AuditWriter, MemorySink, Stamper, WriterConfig, WriterId, WriterIdentity};
use trogon_nats::NatsToken;
use trogon_nats::test_support::TlsJetStreamCluster;
use trogon_std::{SystemClock, UuidV7Generator};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

use crate::{
    ApprovedRegistration, ProducerConfig, ProducerStatus, RegistrationProducer, RegistrationRole, StreamProfile,
    approval_signing_bytes, provision_streams,
};

fn approval(config: &ProducerConfig, revision: u64, issued: SystemTime) -> ApprovedRegistration {
    let snapshot = RegistrationSnapshot {
        tenant: "acme".into(),
        project: "payments".into(),
        recovery_fence: 1,
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
    };
    let payload = snapshot.encode_to_vec();
    let signature = SigningKey::from_bytes(&[7; 32])
        .sign(&approval_signing_bytes(config, &payload))
        .to_bytes();
    ApprovedRegistration::verify(config, &payload, &signature).expect("signed fresh approval")
}

#[tokio::test]
async fn actual_reconnect_during_required_audit_permanently_fences_the_retained_writer() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let config = ProducerConfig::new(
        "acme",
        "payments",
        "identity.example.org",
        NonZeroU64::MIN,
        key.verifying_key(),
    )
    .expect("scope");
    let profile = StreamProfile::new(&config);
    let publisher = NatsToken::new("publisher").expect("publisher");
    let cluster = TlsJetStreamCluster::start(&[(
        publisher.clone(),
        json!({
            "publish": profile.publish_allowlist(RegistrationRole::Publisher),
            "subscribe": profile.subscribe_allowlist(RegistrationRole::Publisher),
        }),
    )])
    .await;
    let admin = cluster
        .config(&NatsToken::new("fixture-admin").expect("admin"))
        .expect("admin config");
    provision_streams(&admin, &config).await.expect("protected streams");
    let sink = MemorySink::new();
    let writer = AuditWriter::new(
        sink.clone(),
        WriterIdentity::new(
            WriterId::new("reconnect-test").expect("writer"),
            NatsToken::new("workload_registration").expect("component"),
            AuditName::new("acme").expect("tenant"),
            NatsToken::new("payments").expect("project"),
        ),
        WriterConfig::default(),
        &Stamper::new(SystemClock, UuidV7Generator),
    );
    let issued = SystemTime::now() - Duration::from_secs(5);
    let mut producer = RegistrationProducer::claim(
        config.clone(),
        cluster.config(&publisher).expect("publisher config"),
        approval(&config, 1, issued),
        writer,
    )
    .await
    .expect("claimed writer");
    // Fault injection uses the real private transport, while behavior is exercised through apply.
    let client = producer.broker.context.client();
    let before = client.server_info();
    let attempts = sink.attempts();
    sink.hold();
    let result = {
        let update = producer.apply(approval(&config, 2, issued + Duration::from_secs(1)));
        tokio::pin!(update);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    result = &mut update => panic!("grant bypassed audit barrier: {result:?}"),
                    () = tokio::time::sleep(Duration::from_millis(10)) => if sink.attempts()>attempts { break; },
                }
            }
        })
        .await
        .expect("grant waits on audit");
        client.force_reconnect().await.expect("inject actual broker reconnect");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let after = client.server_info();
                if client.connection_state() == async_nats::connection::State::Connected
                    && (after.server_id != before.server_id || after.client_id != before.client_id)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual native connection identity changes");
        sink.release();
        tokio::time::timeout(Duration::from_secs(5), &mut update)
            .await
            .expect("writer terminates after audit")
    };
    assert!(result.is_err(), "reconnected ownership cannot publish an old grant");
    assert_eq!(producer.status(), ProducerStatus::Poisoned);
    let latest = producer
        .broker
        .live
        .get_last_raw_message_by_subject(&profile.live_subject())
        .await
        .expect("live state");
    assert_eq!(
        RegistrationSnapshot::decode_from_slice(&latest.payload)
            .expect("snapshot")
            .registration_revision,
        1
    );
}
