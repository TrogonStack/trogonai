#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    num::NonZeroUsize,
    time::{Duration, SystemTime},
};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use ed25519_dalek::{Signer, SigningKey};
use registration_archive_fixture::RegistrationArchiveFixture;
use trogon_audit::{AuditName, AuditWriter, MemorySink, Stamper, WriterConfig, WriterId, WriterIdentity};
use trogon_nats::jetstream::{
    NativeKvExpectation, NativeKvKey, NativeKvScope, NativeKvUpdate, conditional_native_kv_update,
};
use trogon_nats::{NatsToken, connect_once_with_inbox_prefix};
use trogon_std::{SystemClock, UuidV7Generator};
use trogon_workload_registration::{
    ApprovedRegistration, ProducerConfig, RegistrationArchive, RegistrationArchiveCreate, RegistrationArchiveError,
    RegistrationArtifact, RegistrationArtifactInput, RegistrationArtifactKind, RegistrationProducer, RegistrationRole,
    StreamProfile, approval_signing_bytes, provision_streams,
};
use trogonai_proto::platform::workload_identity::v1alpha1::{
    RegistrationBinding, RegistrationSnapshot, StoredRegistrationApprovalV1, WorkloadRole,
};

async fn bounded<T>(operation: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), operation)
        .await
        .expect("original five-second native bound")
}

fn approval(config: &ProducerConfig, revision: u64) -> (ApprovedRegistration, RegistrationArtifact) {
    let now = SystemTime::now();
    let snapshot = RegistrationSnapshot {
        tenant: "acme".into(),
        project: "project".into(),
        recovery_fence: 1,
        registration_revision: revision,
        trust_revision: 1,
        issued_at: MessageField::some(Timestamp::from(now)),
        expires_at: MessageField::some(Timestamp::from(now + Duration::from_secs(120))),
        bindings: vec![RegistrationBinding {
            spiffe_id: "spiffe://identity.example.org/service/egress".into(),
            tenant: "acme".into(),
            project: "project".into(),
            role: EnumValue::from(WorkloadRole::EgressProxy),
            application_id: None,
        }],
        approved_root_sha256: vec![vec![1; 32]],
    };
    let payload = snapshot.encode_to_vec();
    let key = SigningKey::from_bytes(&[7; 32]);
    let signature = key.sign(&approval_signing_bytes(config, &payload));
    key.verifying_key()
        .verify_strict(&approval_signing_bytes(config, &payload), &signature)
        .unwrap();
    let approved = ApprovedRegistration::verify(config, &payload, &signature.to_bytes()).unwrap();
    assert_eq!(approved.payload().as_ref(), payload.as_slice());
    let original: Bytes = StoredRegistrationApprovalV1 {
        approval_payload: payload,
        approval_signature: signature.to_bytes().to_vec(),
    }
    .encode_to_vec()
    .into();
    let artifact = RegistrationArtifact::try_from(RegistrationArtifactInput {
        kind: RegistrationArtifactKind::OriginalApproval,
        bytes: original,
    })
    .unwrap();
    (approved, artifact)
}

fn writer() -> AuditWriter<MemorySink> {
    AuditWriter::new(
        MemorySink::new(),
        WriterIdentity::new(
            WriterId::new("registration-native").unwrap(),
            NatsToken::new("registration_native").unwrap(),
            AuditName::new("acme").unwrap(),
            NatsToken::new("project").unwrap(),
        ),
        WriterConfig::default(),
        &Stamper::new(SystemClock, UuidV7Generator),
    )
}

#[tokio::test]
async fn archive_create_requires_actual_ack_after_native_original_bytes_and_rolling_head_controls() {
    let fixture = RegistrationArchiveFixture::start_with_legacy(16).await;
    let config = fixture.config.clone();
    let profile = &fixture.profile;
    let legacy = StreamProfile::new(&config);
    let legacy_config = fixture.legacy_config();
    let native = &fixture.native;
    let reader = &fixture.reader;
    provision_streams(&fixture.admin_config, &config).await.unwrap();
    let (first, original) = approval(&config, 1);
    let scope = NativeKvScope::new(NatsToken::new(profile.bucket(RegistrationArtifactKind::OriginalApproval)).unwrap())
        .unwrap();
    let key = NativeKvKey::new(original.digest().to_hex()).unwrap();
    let acknowledged = bounded(conditional_native_kv_update(
        native,
        NativeKvUpdate::new(
            &scope,
            &key,
            original.bytes().clone(),
            NativeKvExpectation::from_sequences(0, 0).unwrap(),
            NonZeroUsize::new(RegistrationArtifactKind::OriginalApproval.maximum_message_bytes()).unwrap(),
        ),
    ))
    .await
    .unwrap();
    assert_eq!(acknowledged.get(), 1);
    let stored = bounded(reader.read_kv_sequence(&scope, &key, acknowledged))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.payload, *original.bytes());
    assert_eq!(stored.sequence, acknowledged.get());
    assert_eq!(
        trogon_workload_registration::Sha256Digest::hash(&stored.payload),
        original.digest()
    );
    let mut producer = RegistrationProducer::claim(config.clone(), legacy_config.clone(), first, writer())
        .await
        .unwrap();
    let (second, requested) = approval(&config, 2);
    let rolling = producer.apply(second.clone()).await.unwrap();
    let legacy_client = connect_once_with_inbox_prefix(
        &legacy_config,
        legacy_config.auth.clone(),
        Duration::from_secs(5),
        &legacy.inbox_prefix(RegistrationRole::Publisher).unwrap(),
    )
    .await
    .unwrap();
    let legacy_context = async_nats::jetstream::new(legacy_client);
    for (name, sequence) in [
        (legacy.control_config().name, rolling.control_sequence()),
        (legacy.live_config().name, rolling.live_sequence()),
    ] {
        let stream = bounded(legacy_context.get_stream(name)).await.unwrap();
        assert_eq!(stream.cached_info().state.messages, 1);
        assert_eq!(stream.cached_info().state.last_sequence, sequence.get());
        let live = bounded(stream.get_raw_message(sequence.get())).await.unwrap();
        assert_eq!(live.payload, second.payload());
    }
    let retained = bounded(reader.read_kv_sequence(&scope, &key, acknowledged))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.payload, *original.bytes());
    assert_eq!(retained.sequence, acknowledged.get());
    assert_ne!(original.digest(), requested.digest());
    let archive = &fixture.archive;
    let RegistrationArchiveCreate::Acknowledged(receipt) = archive.create_once(requested.clone()).await.unwrap() else {
        panic!("a distinct never-created digest cannot be an existing observation")
    };
    assert_eq!(receipt.artifact(), &requested);
    assert_eq!(receipt.native_revision().get(), 2);
    let requested_key = NativeKvKey::new(requested.digest().to_hex()).unwrap();
    let actual = bounded(reader.read_kv_sequence(&scope, &requested_key, receipt.native_revision()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.payload, *requested.bytes());
    assert_eq!(actual.sequence, receipt.native_revision().get());
}

#[tokio::test]
async fn archive_only_create_requires_native_ack_and_exact_original_byte_receipt() {
    let fixture = RegistrationArchiveFixture::start(16).await;
    let config = fixture.config.clone();
    let profile = &fixture.profile;
    let native = &fixture.native;
    let reader = &fixture.reader;
    let (_, original) = approval(&config, 1);
    let scope = NativeKvScope::new(NatsToken::new(profile.bucket(RegistrationArtifactKind::OriginalApproval)).unwrap())
        .unwrap();
    let key = NativeKvKey::new(original.digest().to_hex()).unwrap();
    let acknowledged = bounded(conditional_native_kv_update(
        native,
        NativeKvUpdate::new(
            &scope,
            &key,
            original.bytes().clone(),
            NativeKvExpectation::from_sequences(0, 0).unwrap(),
            NonZeroUsize::new(RegistrationArtifactKind::OriginalApproval.maximum_message_bytes()).unwrap(),
        ),
    ))
    .await
    .unwrap();
    assert_eq!(acknowledged.get(), 1);
    let stored = bounded(reader.read_kv_sequence(&scope, &key, acknowledged))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.payload, *original.bytes());
    assert_eq!(stored.sequence, acknowledged.get());
    assert_eq!(
        trogon_workload_registration::Sha256Digest::hash(&stored.payload),
        original.digest()
    );
    let (_, requested) = approval(&config, 2);
    assert_ne!(original.digest(), requested.digest());
    let archive = &fixture.archive;
    let RegistrationArchiveCreate::Acknowledged(receipt) = archive.create_once(requested.clone()).await.unwrap() else {
        panic!("a distinct never-created digest cannot be an existing observation")
    };
    assert_eq!(receipt.artifact(), &requested);
    assert_eq!(receipt.native_revision().get(), 2);
    let requested_key = NativeKvKey::new(requested.digest().to_hex()).unwrap();
    let actual = bounded(reader.read_kv_sequence(&scope, &requested_key, receipt.native_revision()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.payload, *requested.bytes());
    assert_eq!(actual.sequence, receipt.native_revision().get());
}

#[path = "support/registration_archive_fixture.rs"]
mod registration_archive_fixture;

#[tokio::test]
async fn neutral_archive_preserves_ack_bytes_existing_observations_integrity_capacity_and_source_identity() {
    let fixture = RegistrationArchiveFixture::start(4).await;
    let (_, first) = approval(&fixture.config, 1);
    let receipt = match fixture.archive.create_once(first.clone()).await.unwrap() {
        RegistrationArchiveCreate::Acknowledged(receipt) => receipt,
        RegistrationArchiveCreate::ExistingExact(_) => panic!("fresh explicit archive has no original occurrence"),
    };
    assert_eq!(receipt.artifact(), &first);
    assert_eq!(receipt.native_revision().get(), 1);
    let scope = fixture.scope();
    let native_source = bounded(fixture.admin.get_stream(scope.stream())).await.unwrap();
    let source_created_at = SystemTime::from(native_source.cached_info().created);
    assert_eq!(receipt.stream_created_at(), source_created_at);
    assert_eq!(
        receipt.source_profile().stream_config(first.kind()).name,
        native_source.cached_info().config.name
    );
    assert!(std::ptr::eq(receipt.source_profile(), fixture.archive.profile()));
    assert_eq!(format!("{receipt:?}"), "RegistrationArchiveReceipt { .. }");
    let key = NativeKvKey::new(first.digest().to_hex()).unwrap();
    let raw = bounded(fixture.reader.read_kv_sequence(&scope, &key, receipt.native_revision()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.payload, *first.bytes());
    assert_eq!(raw.sequence, receipt.native_revision().get());
    assert_eq!(
        trogon_workload_registration::Sha256Digest::hash(&raw.payload),
        first.digest()
    );
    let observed = fixture
        .archive
        .observe_exact(first.kind(), first.digest())
        .await
        .unwrap();
    assert_eq!(observed.artifact(), &first);
    assert_eq!(observed.native_revision(), receipt.native_revision());
    assert_eq!(observed.stream_created_at(), source_created_at);
    assert!(std::ptr::eq(observed.source_profile(), receipt.source_profile()));
    assert_eq!(format!("{observed:?}"), "RegistrationArtifactObservation { .. }");
    observed.revalidate_original().await.unwrap();
    let before_duplicate = fixture.head().await;
    match fixture.archive.create_once(first.clone()).await.unwrap() {
        RegistrationArchiveCreate::ExistingExact(existing) => {
            assert_eq!(existing.artifact(), &first);
            assert_eq!(existing.native_revision(), receipt.native_revision());
            assert_eq!(existing.stream_created_at(), source_created_at);
            assert!(std::ptr::eq(existing.source_profile(), receipt.source_profile()));
            existing.revalidate_original().await.unwrap();
        }
        RegistrationArchiveCreate::Acknowledged(_) => panic!("an existing exact record cannot synthesize another ACK"),
    }
    assert_eq!(fixture.head().await, before_duplicate);
    let (_, missing) = approval(&fixture.config, 99);
    assert!(matches!(
        fixture.archive.observe_exact(missing.kind(), missing.digest()).await,
        Err(RegistrationArchiveError::Missing)
    ));
    let (_, poisoned_key) = approval(&fixture.config, 2);
    let (_, other_bytes) = approval(&fixture.config, 3);
    assert_ne!(poisoned_key.digest(), other_bytes.digest());
    let poison_revision = fixture
        .publish(
            poisoned_key.digest(),
            other_bytes.bytes().clone(),
            async_nats::HeaderMap::new(),
        )
        .await;
    let poison_raw = bounded(fixture.reader.read_kv_sequence(
        &scope,
        &NativeKvKey::new(poisoned_key.digest().to_hex()).unwrap(),
        poison_revision,
    ))
    .await
    .unwrap()
    .unwrap();
    assert_eq!(poison_raw.payload, *other_bytes.bytes());
    assert!(matches!(
        fixture
            .archive
            .observe_exact(poisoned_key.kind(), poisoned_key.digest())
            .await,
        Err(RegistrationArchiveError::DigestMismatch)
    ));
    assert!(matches!(
        fixture.archive.create_once(poisoned_key).await,
        Err(RegistrationArchiveError::DigestMismatch)
    ));
    assert_eq!(fixture.head().await, poison_revision.get());
    let (_, malformed) = approval(&fixture.config, 4);
    let mut headers = async_nats::HeaderMap::new();
    headers.append("KV-Operation", "DEL");
    headers.append("KV-Operation", "DEL");
    let malformed_revision = fixture
        .publish(malformed.digest(), malformed.bytes().clone(), headers)
        .await;
    let malformed_raw = bounded(fixture.reader.read_kv_sequence(
        &scope,
        &NativeKvKey::new(malformed.digest().to_hex()).unwrap(),
        malformed_revision,
    ))
    .await
    .unwrap()
    .unwrap();
    assert_eq!(malformed_raw.payload, *malformed.bytes());
    let operation_values: usize = malformed_raw
        .headers
        .iter()
        .filter(|(name, _)| AsRef::<str>::as_ref(name).eq_ignore_ascii_case("KV-Operation"))
        .map(|(_, values)| values.len())
        .sum();
    assert_eq!(operation_values, 2);
    assert!(matches!(
        fixture
            .archive
            .observe_exact(malformed.kind(), malformed.digest())
            .await,
        Err(RegistrationArchiveError::NativeOperation(_))
    ));
    let (_, last) = approval(&fixture.config, 5);
    assert!(matches!(
        fixture.archive.create_once(last).await.unwrap(),
        RegistrationArchiveCreate::Acknowledged(_)
    ));
    assert_eq!(fixture.head().await, 4);
    let (_, over_capacity) = approval(&fixture.config, 6);
    assert!(matches!(
        fixture.archive.create_once(over_capacity).await,
        Err(RegistrationArchiveError::CapacityReached)
    ));
    assert_eq!(fixture.head().await, 4);
    let retained = bounded(fixture.reader.read_kv_sequence(&scope, &key, receipt.native_revision()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.payload, *first.bytes());
    assert_eq!(retained.sequence, receipt.native_revision().get());
    observed.revalidate_original().await.unwrap();
    assert!(matches!(
        fixture.archive.create_once(first.clone()).await.unwrap(),
        RegistrationArchiveCreate::ExistingExact(_)
    ));
    assert_eq!(fixture.head().await, 4);
    let manifest = RegistrationArtifact::try_from(RegistrationArtifactInput {
        kind: RegistrationArtifactKind::BootstrapManifest,
        bytes: Bytes::from_static(b"bounded public metadata; not interpreted manifest authority"),
    })
    .unwrap();
    let manifest_receipt = match fixture.archive.create_once(manifest.clone()).await.unwrap() {
        RegistrationArchiveCreate::Acknowledged(receipt) => receipt,
        RegistrationArchiveCreate::ExistingExact(_) => panic!("fresh closed manifest scope has no record"),
    };
    assert_eq!(manifest_receipt.native_revision().get(), 1);
    assert_eq!(manifest_receipt.artifact(), &manifest);
    let manifest_scope = NativeKvScope::new(NatsToken::new(fixture.profile.bucket(manifest.kind())).unwrap()).unwrap();
    assert_ne!(scope, manifest_scope);
    let manifest_raw = bounded(fixture.reader.read_kv_sequence(
        &manifest_scope,
        &NativeKvKey::new(manifest.digest().to_hex()).unwrap(),
        manifest_receipt.native_revision(),
    ))
    .await
    .unwrap()
    .unwrap();
    assert_eq!(manifest_raw.payload, *manifest.bytes());
    fixture
        .archive
        .observe_exact(manifest.kind(), manifest.digest())
        .await
        .unwrap()
        .revalidate_original()
        .await
        .unwrap();
    assert_eq!(fixture.head().await, 4);
    let original_profile = fixture.profile.stream_config(first.kind());
    let mut changed_profile = original_profile.clone();
    changed_profile.max_messages += 1;
    bounded(fixture.admin.update_stream(changed_profile)).await.unwrap();
    assert!(matches!(
        fixture.archive.observe_exact(first.kind(), first.digest()).await,
        Err(RegistrationArchiveError::Profile)
    ));
    assert!(matches!(
        observed.revalidate_original().await,
        Err(RegistrationArchiveError::Profile)
    ));
    bounded(fixture.admin.delete_stream(original_profile.name.clone()))
        .await
        .unwrap();
    let replacement = bounded(fixture.admin.create_stream(original_profile)).await.unwrap();
    assert_ne!(
        SystemTime::from(replacement.cached_info().created),
        observed.stream_created_at()
    );
    assert!(matches!(
        fixture.archive.observe_exact(first.kind(), first.digest()).await,
        Err(RegistrationArchiveError::Incarnation)
    ));
    assert!(matches!(
        observed.revalidate_original().await,
        Err(RegistrationArchiveError::Incarnation)
    ));
    let (_, after_replacement) = approval(&fixture.config, 7);
    assert!(matches!(
        fixture.archive.create_once(after_replacement).await,
        Err(RegistrationArchiveError::Incarnation)
    ));
    let replacement_state = bounded(replacement.get_info()).await.unwrap();
    assert_eq!(replacement_state.state.messages, 0);
    assert_eq!(replacement_state.state.last_sequence, 0);
}

#[tokio::test]
async fn reader_credentials_connect_and_revalidate_both_artifact_kinds() {
    let fixture = RegistrationArchiveFixture::start(4).await;
    for kind in RegistrationArtifactKind::ALL {
        let artifact = RegistrationArtifact::try_from(RegistrationArtifactInput {
            kind,
            bytes: Bytes::from_static(b"original public artifact retained for a reader"),
        })
        .unwrap();
        let RegistrationArchiveCreate::Acknowledged(receipt) =
            fixture.archive.create_once(artifact.clone()).await.unwrap()
        else {
            panic!("fresh artifact must have its own native acknowledgement")
        };
        assert_eq!(receipt.artifact(), &artifact);
    }
    let reader = tokio::time::timeout(
        Duration::from_secs(10),
        RegistrationArchive::connect_reader(fixture.archive_config.clone(), fixture.reader_config.clone()),
    )
    .await
    .expect("bounded archive reader connection")
    .expect("reader credentials must connect using their allowed reply inbox");
    let writer_attempt = tokio::time::timeout(
        Duration::from_secs(10),
        RegistrationArchive::connect(fixture.archive_config.clone(), fixture.reader_config.clone()),
    )
    .await
    .expect("bounded writer connection attempt");
    assert!(
        writer_attempt.is_err(),
        "reader credentials cannot acquire writer reply permissions"
    );
    for kind in RegistrationArtifactKind::ALL {
        let artifact = RegistrationArtifact::try_from(RegistrationArtifactInput {
            kind,
            bytes: Bytes::from_static(b"original public artifact retained for a reader"),
        })
        .unwrap();
        let observed = bounded(reader.observe_exact(kind, artifact.digest())).await.unwrap();
        assert_eq!(observed.artifact(), &artifact);
        bounded(observed.revalidate_original()).await.unwrap();
        let stream = bounded(fixture.admin.get_stream(fixture.profile.stream_config(kind).name))
            .await
            .unwrap();
        let before = stream.cached_info().state.clone();
        assert!(matches!(
            bounded(reader.create_once(artifact)).await,
            Err(RegistrationArchiveError::ReadOnly)
        ));
        let absent = RegistrationArtifact::try_from(RegistrationArtifactInput {
            kind,
            bytes: Bytes::from_static(b"new public artifact a reader cannot create"),
        })
        .unwrap();
        assert!(matches!(
            bounded(reader.create_once(absent)).await,
            Err(RegistrationArchiveError::ReadOnly)
        ));
        let after = bounded(stream.get_info()).await.unwrap().state;
        assert_eq!(after.messages, before.messages);
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.last_sequence, before.last_sequence);
    }
}
