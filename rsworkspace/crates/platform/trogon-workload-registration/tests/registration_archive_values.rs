#![allow(clippy::unwrap_used)]

use std::{collections::BTreeSet, num::NonZeroU64, time::Duration};

use async_nats::jetstream::stream::{Compression, DiscardPolicy, StorageType};
use bytes::Bytes;
use ed25519_dalek::SigningKey;
use trogon_workload_registration::{
    MAX_SNAPSHOT_BYTES, ProducerConfig, RegistrationArchiveCapacity, RegistrationArchiveConfig,
    RegistrationArchiveError, RegistrationArchiveProfile, RegistrationArchiveRole, RegistrationArtifact,
    RegistrationArtifactInput, RegistrationArtifactKind,
};

fn producer(fence: u64, tenant: &str) -> ProducerConfig {
    ProducerConfig::new(
        tenant,
        "project",
        "identity.example.org",
        NonZeroU64::new(fence).unwrap(),
        SigningKey::from_bytes(&[7; 32]).verifying_key(),
    )
    .unwrap()
}

fn archive_config(fence: u64, tenant: &str) -> RegistrationArchiveConfig {
    RegistrationArchiveConfig::new(
        producer(fence, tenant),
        RegistrationArchiveCapacity::new(RegistrationArtifactKind::OriginalApproval, 16, 4 * 1024 * 1024).unwrap(),
        RegistrationArchiveCapacity::new(RegistrationArtifactKind::BootstrapManifest, 16, 4 * 1024 * 1024).unwrap(),
    )
    .unwrap()
}

#[test]
fn archive_inboxes_support_every_recovery_fence_and_keep_role_acl_scoped() {
    let mut inboxes = BTreeSet::new();
    for fence in [999, 1000, 99_999_999, 100_000_000, u64::MAX] {
        let profile = RegistrationArchiveProfile::new(&archive_config(fence, "acme")).unwrap();
        for (role, token) in [
            (RegistrationArchiveRole::Provisioner, "provisioner"),
            (RegistrationArchiveRole::Writer, "writer"),
            (RegistrationArchiveRole::Reader, "reader"),
        ] {
            let inbox = profile.inbox_prefix(role).unwrap();
            assert!(inbox.as_str().len() <= 128);
            assert!(inbox.as_str().starts_with("_INBOX."));
            assert!(inbox.as_str().ends_with(&format!(
                ".{}.{token}",
                profile.bucket(RegistrationArtifactKind::OriginalApproval)
            )));
            assert_eq!(profile.subscribe_allowlist(role).unwrap(), vec![format!("{inbox}.>")]);
            assert!(inboxes.insert(inbox.as_str().to_owned()));
        }
    }
    assert_eq!(inboxes.len(), 15);
}
#[test]
fn archive_artifacts_enforce_kind_bounds_preserve_bytes_and_redact_debug() {
    for kind in RegistrationArtifactKind::ALL {
        assert!(matches!(
            RegistrationArtifact::try_from(RegistrationArtifactInput {
                kind,
                bytes: Bytes::new(),
            }),
            Err(RegistrationArchiveError::ArtifactBounds)
        ));
        let maximum = kind.maximum_bytes();
        let bytes = Bytes::from(vec![b'x'; maximum]);
        let artifact = RegistrationArtifact::try_from(RegistrationArtifactInput {
            kind,
            bytes: bytes.clone(),
        })
        .unwrap();
        assert_eq!(artifact.kind(), kind);
        assert_eq!(artifact.bytes(), &bytes);
        assert!(matches!(
            RegistrationArtifact::try_from(RegistrationArtifactInput {
                kind,
                bytes: Bytes::from(vec![b'x'; maximum + 1]),
            }),
            Err(RegistrationArchiveError::ArtifactBounds)
        ));

        let known = RegistrationArtifact::try_from(RegistrationArtifactInput {
            kind,
            bytes: Bytes::from_static(b"abc"),
        })
        .unwrap();
        assert_eq!(
            known.digest().to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(known.bytes().as_ref(), b"abc");
        let rendered = format!("{known:?}");
        assert!(rendered.contains("RegistrationArtifact"));
        assert!(rendered.contains("byte_count: 3"));
        assert!(!rendered.contains("abc"));
        assert!(!rendered.contains(&known.digest().to_hex()));
    }
    assert_eq!(
        RegistrationArtifactKind::OriginalApproval.maximum_bytes(),
        MAX_SNAPSHOT_BYTES + 70
    );
    assert_eq!(RegistrationArtifactKind::BootstrapManifest.maximum_bytes(), 64 * 1024);
    for kind in RegistrationArtifactKind::ALL {
        assert_eq!(kind.maximum_message_bytes(), kind.maximum_bytes() + 16 * 1024);
    }
}

#[test]
fn archive_capacities_reject_zero_overflow_and_insufficient_message_space() {
    let largest = u64::try_from(i64::MAX).unwrap();
    for kind in RegistrationArtifactKind::ALL {
        let minimum = u64::try_from(kind.maximum_message_bytes()).unwrap();
        for records in [0, largest + 1, u64::MAX] {
            assert!(matches!(
                RegistrationArchiveCapacity::new(kind, records, minimum),
                Err(RegistrationArchiveError::Capacity)
            ));
        }
        for bytes in [0, minimum - 1, largest + 1, u64::MAX] {
            assert!(matches!(
                RegistrationArchiveCapacity::new(kind, 1, bytes),
                Err(RegistrationArchiveError::Capacity)
            ));
        }
        let exact = RegistrationArchiveCapacity::new(kind, 1, minimum).unwrap();
        assert_eq!(exact.kind(), kind);
        assert_eq!(exact.records(), 1);
        assert_eq!(exact.bytes(), i64::try_from(minimum).unwrap());
        let largest_capacity = RegistrationArchiveCapacity::new(kind, largest, largest).unwrap();
        assert_eq!(largest_capacity.kind(), kind);
        assert_eq!(largest_capacity.records(), i64::MAX);
        assert_eq!(largest_capacity.bytes(), i64::MAX);
    }
}

#[test]
fn archive_config_keeps_approval_and_manifest_capacity_owners_distinct() {
    let registration = producer(1, "acme");
    let approval =
        RegistrationArchiveCapacity::new(RegistrationArtifactKind::OriginalApproval, 3, 2 * 1024 * 1024).unwrap();
    let manifest =
        RegistrationArchiveCapacity::new(RegistrationArtifactKind::BootstrapManifest, 7, 1024 * 1024).unwrap();
    for (first, second) in [(manifest, approval), (approval, approval), (manifest, manifest)] {
        assert!(matches!(
            RegistrationArchiveConfig::new(registration.clone(), first, second),
            Err(RegistrationArchiveError::Capacity)
        ));
    }
    let config = RegistrationArchiveConfig::new(registration.clone(), approval, manifest).unwrap();
    assert_eq!(config.registration(), &registration);
    assert_eq!(config.capacity(RegistrationArtifactKind::OriginalApproval), approval);
    assert_eq!(config.capacity(RegistrationArtifactKind::BootstrapManifest), manifest);
}

#[test]
fn archive_storage_names_retain_full_scope_fingerprints_at_every_fence() {
    let mut names = BTreeSet::new();
    for (tenant, fingerprint) in [
        (
            "acme",
            "f392db31daac7fbb8abcf463f9bc4c3b540b170e81a359537733905a12c9cc9c",
        ),
        (
            "other",
            "e5e65791e35b36d5c72fe3e512e2ff5d7d20d5cf3f6c803d455dfaac14683cd4",
        ),
    ] {
        for fence in [999, 1000, 99_999_999, 100_000_000, u64::MAX] {
            let profile = RegistrationArchiveProfile::new(&archive_config(fence, tenant)).unwrap();
            for (kind, suffix) in [
                (RegistrationArtifactKind::OriginalApproval, "approval"),
                (RegistrationArtifactKind::BootstrapManifest, "manifest"),
            ] {
                let expected = format!("wr_archive_{fingerprint}_{fence}_{suffix}");
                assert_eq!(profile.bucket(kind), expected);
                assert_eq!(profile.stream_config(kind).name, format!("KV_{expected}"));
                assert!(names.insert(expected));
            }
        }
    }
    assert_eq!(names.len(), 20);
}

#[test]
fn archive_profiles_keep_immutable_storage_and_least_privilege_role_permissions() {
    let config = archive_config(u64::MAX, "acme");
    let profile = RegistrationArchiveProfile::new(&config).unwrap();
    for kind in RegistrationArtifactKind::ALL {
        let stream = profile.stream_config(kind);
        assert_eq!(stream.subjects, vec![format!("$KV.{}.*", profile.bucket(kind))]);
        assert_eq!(stream.num_replicas, 3);
        assert_eq!(stream.storage, StorageType::File);
        assert_eq!(stream.compression, Some(Compression::None));
        assert_eq!(stream.discard, DiscardPolicy::New);
        assert!(stream.discard_new_per_subject);
        assert!(stream.deny_delete);
        assert!(stream.deny_purge);
        assert!(!stream.allow_rollup);
        assert!(!stream.sealed);
        assert_eq!(stream.max_age, Duration::ZERO);
        assert_eq!(stream.max_messages_per_subject, 1);
        assert_eq!(stream.max_messages, config.capacity(kind).records());
        assert_eq!(stream.max_bytes, config.capacity(kind).bytes());
        assert_eq!(
            stream.max_message_size,
            i32::try_from(kind.maximum_message_bytes()).unwrap()
        );
        assert!(stream.mirror.is_none());
        assert!(stream.sources.is_none());
        assert!(stream.republish.is_none());
    }
    let approval = profile.stream_config(RegistrationArtifactKind::OriginalApproval).name;
    let manifest = profile.stream_config(RegistrationArtifactKind::BootstrapManifest).name;
    assert_eq!(
        profile.publish_allowlist(RegistrationArchiveRole::Provisioner),
        vec![
            format!("$JS.API.STREAM.INFO.{approval}"),
            format!("$JS.API.STREAM.CREATE.{approval}"),
            format!("$JS.API.STREAM.INFO.{manifest}"),
            format!("$JS.API.STREAM.CREATE.{manifest}"),
        ]
    );
    assert_eq!(
        profile.publish_allowlist(RegistrationArchiveRole::Writer),
        vec![
            format!("$JS.API.STREAM.INFO.{approval}"),
            format!("$JS.API.STREAM.MSG.GET.{approval}"),
            format!("$KV.{}.*", profile.bucket(RegistrationArtifactKind::OriginalApproval)),
            format!("$JS.API.STREAM.INFO.{manifest}"),
            format!("$JS.API.STREAM.MSG.GET.{manifest}"),
            format!("$KV.{}.*", profile.bucket(RegistrationArtifactKind::BootstrapManifest)),
        ]
    );
    assert_eq!(
        profile.publish_allowlist(RegistrationArchiveRole::Reader),
        vec![
            format!("$JS.API.STREAM.INFO.{approval}"),
            format!("$JS.API.STREAM.MSG.GET.{approval}"),
            format!("$JS.API.STREAM.INFO.{manifest}"),
            format!("$JS.API.STREAM.MSG.GET.{manifest}"),
        ]
    );
}
