use std::str::FromStr;
use std::time::{Duration, SystemTime};

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    ArtifactType, CorrelationId, Digest, DigestMismatch, DigestMismatchError, EvictedArtifactAction, LeaseFailed,
    LeaseFailedReason, LeaseId, LiveSnapshotEntry, RetentionPolicy, RetentionTagLimit, SnapshotAccountingDisposition,
    SnapshotAccountingRefusal, SnapshotAccountingResponse, SnapshotCaptureStatus, SnapshotDeleted, SnapshotGeneration,
    SnapshotGenerationError, SnapshotPublished, SnapshotPublishedTag, SnapshotRestoreResult, SnapshotTagName, TenantId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIGEST_A: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DIGEST_B: &str = "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const GENERATION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn time(secs: u64) -> serde_json::Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn snapshot_generations_accept_canonical_ulids() -> TestResult {
    for value in [GENERATION, "00000000000000000000000000", "7ZZZZZZZZZZZZZZZZZZZZZZZZZ"] {
        let generation = SnapshotGeneration::new(value)?;
        assert_eq!(generation.as_str(), value);
        assert_eq!(generation.to_string(), value);
        assert_eq!(SnapshotGeneration::from_str(value)?, generation);
        assert_eq!(SnapshotGeneration::try_from(value.to_owned())?, generation);
        assert_eq!(String::from(generation.clone()), value);
        assert_eq!(serde_json::to_value(&generation)?, json!(value));
        assert_eq!(serde_json::from_value::<SnapshotGeneration>(json!(value))?, generation);
    }
    assert_eq!(SnapshotGeneration::LEN, 26);
    Ok(())
}

#[test]
fn snapshot_generations_reject_anything_else() -> TestResult {
    for (value, expected) in [
        ("", SnapshotGenerationError::Length { expected: 26 }),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FA",
            SnapshotGenerationError::Length { expected: 26 },
        ),
        (
            "01ARZ3NDEKTSV4RRFFQ69G5FAVX",
            SnapshotGenerationError::Length { expected: 26 },
        ),
        ("01arz3ndektsv4rrffq69g5fav", SnapshotGenerationError::InvalidCharacter),
        ("01ARZ3NDEKTSV4RRFFQ69G5FAI", SnapshotGenerationError::InvalidCharacter),
        ("01ARZ3NDEKTSV4RRFFQ69G5FAL", SnapshotGenerationError::InvalidCharacter),
        ("01ARZ3NDEKTSV4RRFFQ69G5FAO", SnapshotGenerationError::InvalidCharacter),
        ("01ARZ3NDEKTSV4RRFFQ69G5FAU", SnapshotGenerationError::InvalidCharacter),
        ("01ARZ3NDEKTSV4RRFFQ69G5FA-", SnapshotGenerationError::InvalidCharacter),
        ("81ARZ3NDEKTSV4RRFFQ69G5FAV", SnapshotGenerationError::Overflow),
        ("ZZZZZZZZZZZZZZZZZZZZZZZZZZ", SnapshotGenerationError::Overflow),
    ] {
        assert_eq!(SnapshotGeneration::new(value).err(), Some(expected), "{value}");
        let error = serde_json::from_value::<SnapshotGeneration>(json!(value))
            .err()
            .ok_or("invalid generation deserialized")?;
        assert_eq!(error.classify(), Data, "{value}");
    }
    Ok(())
}

#[test]
fn snapshot_generation_errors_describe_the_rule() {
    assert_eq!(
        SnapshotGenerationError::Length { expected: 26 }.to_string(),
        "snapshot generation must be 26 characters"
    );
    assert_eq!(
        SnapshotGenerationError::InvalidCharacter.to_string(),
        "snapshot generation must use uppercase Crockford base32 characters"
    );
    assert_eq!(
        SnapshotGenerationError::Overflow.to_string(),
        "snapshot generation must start with 0 through 7 to fit in 128 bits"
    );
}

#[test]
fn snapshot_accounting_responses_are_tagged_by_outcome() -> TestResult {
    let committed = SnapshotAccountingResponse::Committed {
        tenant: TenantId::new("acme")?,
        digest: Digest::new(DIGEST_A)?,
        lease_id: LeaseId::new("lease-1"),
        generation: SnapshotGeneration::new(GENERATION)?,
        disposition: SnapshotAccountingDisposition::Live,
    };
    let wire = json!({
        "outcome": "committed",
        "tenant": "acme",
        "digest": DIGEST_A,
        "lease_id": "lease-1",
        "generation": GENERATION,
        "disposition": "live",
    });
    assert_eq!(serde_json::to_value(&committed)?, wire);
    assert_eq!(serde_json::from_value::<SnapshotAccountingResponse>(wire)?, committed);

    let refused = SnapshotAccountingResponse::Refused {
        reason: SnapshotAccountingRefusal::QuotaExceeded,
    };
    let wire = json!({ "outcome": "refused", "reason": "quota_exceeded" });
    assert_eq!(serde_json::to_value(&refused)?, wire);
    assert_eq!(serde_json::from_value::<SnapshotAccountingResponse>(wire)?, refused);

    let bad_generation = json!({
        "outcome": "committed",
        "tenant": "acme",
        "digest": DIGEST_A,
        "lease_id": "lease-1",
        "generation": "not-a-ulid",
        "disposition": "live",
    });
    assert!(serde_json::from_value::<SnapshotAccountingResponse>(bad_generation).is_err());
    Ok(())
}

#[test]
fn snapshot_accounting_enums_use_snake_case_names() -> TestResult {
    for (disposition, name) in [
        (SnapshotAccountingDisposition::Live, "live"),
        (SnapshotAccountingDisposition::Superseded, "superseded"),
        (SnapshotAccountingDisposition::Deleted, "deleted"),
    ] {
        assert_eq!(serde_json::to_value(disposition)?, json!(name));
        assert_eq!(
            serde_json::from_value::<SnapshotAccountingDisposition>(json!(name))?,
            disposition
        );
    }
    for (refusal, name) in [
        (SnapshotAccountingRefusal::LegacyUnconfirmed, "legacy_unconfirmed"),
        (SnapshotAccountingRefusal::UnknownGeneration, "unknown_generation"),
        (SnapshotAccountingRefusal::OwnershipMismatch, "ownership_mismatch"),
        (SnapshotAccountingRefusal::MetadataMismatch, "metadata_mismatch"),
        (SnapshotAccountingRefusal::Capacity, "capacity"),
        (SnapshotAccountingRefusal::QuotaExceeded, "quota_exceeded"),
        (SnapshotAccountingRefusal::Pending, "pending"),
        (SnapshotAccountingRefusal::Backend, "backend"),
    ] {
        assert_eq!(serde_json::to_value(refusal)?, json!(name));
        assert_eq!(
            serde_json::from_value::<SnapshotAccountingRefusal>(json!(name))?,
            refusal
        );
    }
    Ok(())
}

fn published() -> Result<SnapshotPublished, Box<dyn std::error::Error>> {
    Ok(SnapshotPublished {
        lease_id: LeaseId::new("lease-1"),
        digest: Digest::new(DIGEST_A)?,
        artifact_type: ArtifactType::SandboxRootfsV1,
        tenant: TenantId::new("acme")?,
        bytes: 4_096,
        generation: Some(SnapshotGeneration::new(GENERATION)?),
        correlation_id: Some(CorrelationId::new("corr-1")),
        tag: Some(SnapshotPublishedTag {
            name: SnapshotTagName::new("latest")?,
            set_at: at(1_700_000_000),
            retention: Some(RetentionPolicy {
                max_tags: RetentionTagLimit::new(3)?,
                max_age_days: 7,
                delete_evicted: EvictedArtifactAction::Delete,
            }),
        }),
    })
}

#[test]
fn snapshot_published_round_trips_through_json() -> TestResult {
    let original = published()?;
    let wire = serde_json::to_value(&original)?;
    assert_eq!(
        wire,
        json!({
            "lease_id": "lease-1",
            "digest": DIGEST_A,
            "artifact_type": "sandbox_rootfs_v1",
            "tenant": "acme",
            "bytes": 4_096,
            "generation": GENERATION,
            "correlation_id": "corr-1",
            "tag": {
                "name": "latest",
                "set_at": time(1_700_000_000),
                "retention": { "max_tags": 3, "max_age_days": 7, "delete_evicted": true },
            },
        })
    );
    assert_eq!(serde_json::from_value::<SnapshotPublished>(wire)?, original);
    Ok(())
}

#[test]
fn snapshot_published_reads_older_builders_and_rejects_invalid_tags() -> TestResult {
    let older = json!({
        "lease_id": "lease-1",
        "digest": DIGEST_A,
        "artifact_type": "sandbox_rootfs_v1",
        "tenant": "acme",
        "bytes": 4_096,
    });
    let parsed: SnapshotPublished = serde_json::from_value(older.clone())?;
    assert_eq!(parsed.generation, None);
    assert_eq!(parsed.correlation_id, None);
    assert_eq!(parsed.tag, None);
    assert_eq!(serde_json::to_value(&parsed)?, older);

    let tag: SnapshotPublishedTag = serde_json::from_value(json!({ "name": "latest", "set_at": time(5) }))?;
    assert_eq!(tag.retention, None);
    assert!(serde_json::to_value(&tag)?.get("retention").is_none());

    let mut invalid = serde_json::to_value(published()?)?;
    invalid["tag"]["name"] = json!("not a tag");
    let error = serde_json::from_value::<SnapshotPublished>(invalid)
        .err()
        .ok_or("invalid tag name deserialized")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn snapshot_deletions_and_live_entries_round_trip() -> TestResult {
    let deleted = SnapshotDeleted {
        digest: Digest::new(DIGEST_A)?,
        tenant: TenantId::new("acme")?,
        bytes: 10,
        generation: None,
    };
    let wire = json!({ "digest": DIGEST_A, "tenant": "acme", "bytes": 10 });
    assert_eq!(serde_json::to_value(&deleted)?, wire);
    assert_eq!(serde_json::from_value::<SnapshotDeleted>(wire)?, deleted);

    let entry = LiveSnapshotEntry {
        tenant: TenantId::new("acme")?,
        artifact_type: ArtifactType::SbomV1,
        published_at: at(42),
        bytes: 20,
        generation: Some(SnapshotGeneration::new(GENERATION)?),
    };
    let wire = json!({
        "tenant": "acme",
        "artifact_type": "sbom_v1",
        "published_at": time(42),
        "bytes": 20,
        "generation": GENERATION,
    });
    assert_eq!(serde_json::to_value(&entry)?, wire);
    assert_eq!(serde_json::from_value::<LiveSnapshotEntry>(wire)?, entry);

    let restored = SnapshotRestoreResult {
        digest: Digest::new(DIGEST_B)?,
    };
    assert_eq!(serde_json::to_value(&restored)?, json!({ "digest": DIGEST_B }));
    assert_eq!(
        serde_json::from_value::<SnapshotRestoreResult>(json!({ "digest": DIGEST_B }))?,
        restored
    );
    Ok(())
}

#[test]
fn snapshot_capture_statuses_are_tagged_by_kind() -> TestResult {
    let created = SnapshotCaptureStatus::Created {
        digest: Digest::new(DIGEST_A)?,
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    let wire = json!({ "kind": "created", "digest": DIGEST_A, "artifact_type": "sandbox_rootfs_v1" });
    assert_eq!(serde_json::to_value(&created)?, wire);
    assert_eq!(serde_json::from_value::<SnapshotCaptureStatus>(wire)?, created);

    let failed = SnapshotCaptureStatus::Failed {
        reason: "disk full".to_owned(),
    };
    let wire = json!({ "kind": "failed", "reason": "disk full" });
    assert_eq!(serde_json::to_value(&failed)?, wire);
    assert_eq!(serde_json::from_value::<SnapshotCaptureStatus>(wire)?, failed);
    Ok(())
}

#[test]
fn lease_failures_carry_a_real_digest_mismatch() -> TestResult {
    let mismatch = DigestMismatch::new(Digest::new(DIGEST_A)?, Digest::new(DIGEST_B)?)?;
    assert_eq!(mismatch.expected().as_str(), DIGEST_A);
    assert_eq!(mismatch.actual().as_str(), DIGEST_B);

    let failed = LeaseFailed {
        lease_id: LeaseId::new("lease-1"),
        tenant: TenantId::new("acme")?,
        reason: LeaseFailedReason::DigestMismatch(mismatch),
    };
    let wire = json!({
        "lease_id": "lease-1",
        "tenant": "acme",
        "reason": { "kind": "digest_mismatch", "expected": DIGEST_A, "actual": DIGEST_B },
    });
    assert_eq!(serde_json::to_value(&failed)?, wire);
    assert_eq!(serde_json::from_value::<LeaseFailed>(wire)?, failed);
    Ok(())
}

#[test]
fn digest_mismatches_reject_equal_digests() -> TestResult {
    assert_eq!(
        DigestMismatch::new(Digest::new(DIGEST_A)?, Digest::new(DIGEST_A)?).err(),
        Some(DigestMismatchError::SameDigest)
    );
    assert_eq!(
        DigestMismatchError::SameDigest.to_string(),
        "a digest mismatch needs two different digests"
    );
    let same = json!({ "kind": "digest_mismatch", "expected": DIGEST_A, "actual": DIGEST_A });
    let error = serde_json::from_value::<LeaseFailedReason>(same)
        .err()
        .ok_or("equal digests deserialized")?;
    assert_eq!(error.classify(), Data);
    assert!(serde_json::from_value::<DigestMismatch>(json!({ "expected": DIGEST_A })).is_err());
    let partial = json!({ "kind": "digest_mismatch", "expected": DIGEST_A });
    assert!(serde_json::from_value::<LeaseFailedReason>(partial).is_err());
    Ok(())
}
