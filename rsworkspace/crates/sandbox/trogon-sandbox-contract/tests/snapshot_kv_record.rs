use std::time::{Duration, SystemTime};

use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{ArtifactType, Digest, EnvDigest, LeaseId, SnapshotKvRecord, TenantId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn hash(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

fn minimal() -> Result<SnapshotKvRecord, Box<dyn std::error::Error>> {
    Ok(SnapshotKvRecord {
        lease_id: LeaseId::new("lease-1"),
        tenant: TenantId::new("acme")?,
        digest: Digest::new(hash('a'))?,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
        registry: "ghcr.io".to_owned(),
        repository: "snapshots/acme".to_owned(),
        parent: None,
        created_at: None,
        env_digest: None,
    })
}

fn minimal_json() -> Value {
    json!({
        "lease_id": "lease-1",
        "tenant": "acme",
        "digest": hash('a'),
        "artifact_type": "firecracker_snapshot_v1",
        "registry": "ghcr.io",
        "repository": "snapshots/acme"
    })
}

#[test]
fn optional_fields_are_omitted_when_absent() -> TestResult {
    let record = minimal()?;

    assert_eq!(serde_json::to_value(&record)?, minimal_json());
    assert_eq!(serde_json::from_value::<SnapshotKvRecord>(minimal_json())?, record);
    Ok(())
}

#[test]
fn full_record_round_trips() -> TestResult {
    let record = SnapshotKvRecord {
        parent: Some(Digest::new(hash('b'))?),
        created_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
        env_digest: Some(EnvDigest::new(hash('c'))?),
        ..minimal()?
    };
    let mut expected = minimal_json();
    expected["parent"] = json!(hash('b'));
    expected["created_at"] = json!({ "secs_since_epoch": 1_700_000_000u64, "nanos_since_epoch": 0 });
    expected["env_digest"] = json!(hash('c'));

    assert_eq!(serde_json::to_value(&record)?, expected);
    assert_eq!(serde_json::from_value::<SnapshotKvRecord>(expected)?, record);
    Ok(())
}

#[test]
fn an_invalid_environment_digest_is_rejected() -> TestResult {
    let mut value = minimal_json();
    value["env_digest"] = json!("nope");

    let error = serde_json::from_value::<SnapshotKvRecord>(value)
        .err()
        .ok_or("expected error")?;

    assert_eq!(error.classify(), Data);
    Ok(())
}
