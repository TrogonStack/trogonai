use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use trogon_sandbox_contract::{
    ArtifactType, Digest, EvictedArtifactAction, RetentionPolicy, RetentionTagLimit, ServiceAccountId, SnapshotTag,
    SnapshotTagAuditEvent, SnapshotTagName, SnapshotTagNameError, SnapshotTagRecord, TenantId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn digest(hex: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", hex.to_string().repeat(64)))?)
}

fn instant() -> SystemTime {
    UNIX_EPOCH + Duration::new(1_700_000_000, 5)
}

fn snapshot_tag(name: &str) -> Result<SnapshotTag, Box<dyn std::error::Error>> {
    Ok(SnapshotTag {
        tenant: TenantId::new("acme")?,
        name: SnapshotTagName::new(name)?,
    })
}

#[test]
fn snapshot_tag_name_accepts_valid_names() -> TestResult {
    let longest = "a".repeat(SnapshotTagName::MAX_LEN);
    for value in ["prod", "v1.2", "A_b-c.9", longest.as_str()] {
        let name = SnapshotTagName::new(value)?;
        assert_eq!(name.as_str(), value);
        assert_eq!(name.to_string(), value);
        assert_eq!(SnapshotTagName::from_str(value)?, name);
        assert_eq!(SnapshotTagName::try_from(value.to_owned())?, name);
        assert_eq!(serde_json::to_value(&name)?, json!(value));
        assert_eq!(serde_json::from_value::<SnapshotTagName>(json!(value))?, name);
        assert_eq!(String::from(name), value);
    }
    Ok(())
}

#[test]
fn snapshot_tag_name_rejects_invalid_names_at_construction_and_deserialization() {
    let too_long = "a".repeat(SnapshotTagName::MAX_LEN + 1);
    for (value, expected) in [
        ("", SnapshotTagNameError::InvalidLength),
        (too_long.as_str(), SnapshotTagNameError::InvalidLength),
        ("bad/name", SnapshotTagNameError::InvalidCharacters),
        ("has space", SnapshotTagNameError::InvalidCharacters),
        ("caf\u{e9}", SnapshotTagNameError::InvalidCharacters),
    ] {
        assert_eq!(SnapshotTagName::new(value), Err(expected), "{value:?}");
        let error = serde_json::from_value::<SnapshotTagName>(json!(value)).err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some(expected.to_string()),
            "{value:?}"
        );
    }
    assert_eq!(
        SnapshotTagNameError::InvalidLength.to_string(),
        "tag name must be 1-64 characters"
    );
    assert_eq!(
        SnapshotTagNameError::InvalidCharacters.to_string(),
        "tag name must match [a-zA-Z0-9._-]"
    );
}

#[test]
fn snapshot_tag_roundtrips_and_rejects_an_invalid_name() -> TestResult {
    let tag = snapshot_tag("prod")?;
    assert_eq!(tag.tenant.as_str(), "acme");
    let wire = serde_json::to_value(&tag)?;
    assert_eq!(wire, json!({ "tenant": "acme", "name": "prod" }));
    assert_eq!(serde_json::from_value::<SnapshotTag>(wire)?, tag);

    let error = serde_json::from_value::<SnapshotTag>(json!({ "tenant": "acme", "name": "bad/name" })).err();
    assert_eq!(
        error.map(|error| error.to_string()),
        Some(SnapshotTagNameError::InvalidCharacters.to_string())
    );
    Ok(())
}

#[test]
fn snapshot_tag_record_roundtrips_with_and_without_retention() -> TestResult {
    let pinned = digest('a')?;
    let record = SnapshotTagRecord {
        digest: pinned.clone(),
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
        set_by: ServiceAccountId::new("svc-ci"),
        set_at: instant(),
        retention: Some(RetentionPolicy {
            max_tags: RetentionTagLimit::new(3)?,
            max_age_days: 30,
            delete_evicted: EvictedArtifactAction::Delete,
        }),
    };
    let wire = serde_json::to_value(&record)?;
    assert_eq!(
        wire,
        json!({
            "digest": pinned.as_str(),
            "artifact_type": "firecracker_snapshot_v1",
            "set_by": "svc-ci",
            "set_at": { "secs_since_epoch": 1_700_000_000u64, "nanos_since_epoch": 5 },
            "retention": { "max_tags": 3, "max_age_days": 30, "delete_evicted": true },
        })
    );
    assert_eq!(serde_json::from_value::<SnapshotTagRecord>(wire)?, record);

    let bare = SnapshotTagRecord {
        retention: None,
        ..record
    };
    let wire = serde_json::to_value(&bare)?;
    assert!(wire.get("retention").is_none());
    assert_eq!(serde_json::from_value::<SnapshotTagRecord>(wire)?, bare);
    Ok(())
}

#[test]
fn snapshot_tag_audit_events_are_tagged_by_event() -> TestResult {
    let tag = snapshot_tag("prod")?;
    let first = digest('a')?;
    let second = digest('b')?;

    let set = SnapshotTagAuditEvent::Set {
        tag: tag.clone(),
        prev: None,
        next: first.clone(),
        by: ServiceAccountId::new("svc-ci"),
        at: instant(),
    };
    let wire = serde_json::to_value(&set)?;
    assert_eq!(
        wire,
        json!({
            "event": "set",
            "tag": { "tenant": "acme", "name": "prod" },
            "prev": null,
            "next": first.as_str(),
            "by": "svc-ci",
            "at": { "secs_since_epoch": 1_700_000_000u64, "nanos_since_epoch": 5 },
        })
    );
    assert_eq!(serde_json::from_value::<SnapshotTagAuditEvent>(wire)?, set);

    let moved = SnapshotTagAuditEvent::Set {
        tag: tag.clone(),
        prev: Some(first.clone()),
        next: second.clone(),
        by: ServiceAccountId::new("svc-ci"),
        at: instant(),
    };
    let wire = serde_json::to_value(&moved)?;
    assert_eq!(wire["prev"], json!(first.as_str()));
    assert_eq!(serde_json::from_value::<SnapshotTagAuditEvent>(wire)?, moved);

    let cleared = SnapshotTagAuditEvent::Cleared {
        tag,
        prev: second.clone(),
        by: ServiceAccountId::new("svc-ops"),
        at: instant(),
    };
    let wire = serde_json::to_value(&cleared)?;
    assert_eq!(
        wire,
        json!({
            "event": "cleared",
            "tag": { "tenant": "acme", "name": "prod" },
            "prev": second.as_str(),
            "by": "svc-ops",
            "at": { "secs_since_epoch": 1_700_000_000u64, "nanos_since_epoch": 5 },
        })
    );
    assert_eq!(serde_json::from_value::<SnapshotTagAuditEvent>(wire)?, cleared);
    Ok(())
}
