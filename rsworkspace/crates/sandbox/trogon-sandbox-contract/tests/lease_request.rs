use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CapabilityRequirements, CorrelationId, LeaseName, LeasePersistence, LeasePrewarm, LeaseRecord, LeaseRequest,
    ProviderDispatchPayload, ProviderKind, SandboxTags,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn time(secs: u64) -> Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

fn spec() -> Value {
    json!({
        "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
        "workspace": { "empty": {} },
        "cpu_seconds": 10,
        "memory_mib": 512,
        "env": {},
        "privileged": false,
        "boot_mode": "entrypoint_default",
    })
}

/// Parses `wire`, checks it serializes back to exactly the same JSON, and returns the value.
fn canonical<T>(wire: Value) -> Result<T, Box<dyn std::error::Error>>
where
    T: Debug + Serialize + DeserializeOwned,
{
    let parsed = serde_json::from_value::<T>(wire.clone())?;
    assert_eq!(serde_json::to_value(&parsed)?, wire);
    Ok(parsed)
}

/// Parses `input` and checks it serializes to `expected`, which spells out every defaulted field.
fn normalized<T>(input: Value, expected: Value) -> Result<T, Box<dyn std::error::Error>>
where
    T: Debug + Serialize + DeserializeOwned,
{
    let parsed = serde_json::from_value::<T>(input)?;
    assert_eq!(serde_json::to_value(&parsed)?, expected);
    Ok(parsed)
}

fn minimal_request() -> Value {
    json!({
        "tenant": "acme",
        "provider_pref": ["mock", "daytona"],
        "spec": spec(),
        "ttl_seconds": 60,
        "idempotency_key": "key-1",
    })
}

fn defaulted_request() -> Value {
    let mut wire = minimal_request();
    wire["prewarm"] = json!(false);
    wire["tags"] = json!({});
    wire["persistent"] = json!(true);
    wire
}

fn full_request() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(json!({
        "tenant": "acme",
        "provider_pref": ["modal"],
        "spec": spec(),
        "ttl_seconds": 3600,
        "idempotency_key": "key-1",
        "isolation_tier": "micro_vm",
        "host_imports": [{ "kind": "fetch_url", "allow_domains": ["example.com"], "max_body_bytes": 1024, "timeout_seconds": 5 }],
        "required_capabilities": serde_json::to_value(CapabilityRequirements::default())?,
        "prewarm": true,
        "tags": { "team": "core" },
        "name": "dev",
        "persistent": false,
        "native": { "daytona": { "auto_stop_interval": 30 } },
        "region": "us-east",
        "fallback_regions": ["us-west", "eu-west"],
        "operation_id": "op-1",
    }))
}

#[test]
fn a_minimal_request_gets_the_documented_defaults() -> TestResult {
    let request = normalized::<LeaseRequest>(minimal_request(), defaulted_request())?;
    assert_eq!(request.provider_pref, vec![ProviderKind::Mock, ProviderKind::Daytona]);
    assert_eq!(request.prewarm, LeasePrewarm::Regular);
    assert!(!request.prewarm.is_prewarm());
    assert_eq!(request.persistent, LeasePersistence::Persistent);
    assert!(request.persistent.is_persistent());
    assert_eq!(request.tags, SandboxTags::default());
    assert_eq!(request.name, None);
    assert_eq!(request.native, None);
    assert_eq!(request.region, None);
    assert!(request.fallback_regions.is_empty());
    assert!(request.host_imports.is_empty());
    assert_eq!(request.operation_id, None);
    Ok(())
}

#[test]
fn a_full_request_round_trips_exactly() -> TestResult {
    let request = canonical::<LeaseRequest>(full_request()?)?;
    assert_eq!(request.prewarm, LeasePrewarm::Prewarm);
    assert!(request.prewarm.is_prewarm());
    assert_eq!(request.persistent, LeasePersistence::Ephemeral);
    assert!(!request.persistent.is_persistent());
    assert_eq!(request.name, Some(LeaseName::new("dev")?));
    assert_eq!(request.native, Some(json!({ "daytona": { "auto_stop_interval": 30 } })));
    assert_eq!(request.region.as_deref(), Some("us-east"));
    assert_eq!(
        request.fallback_regions,
        vec!["us-west".to_owned(), "eu-west".to_owned()]
    );
    Ok(())
}

#[test]
fn the_flag_enums_are_json_booleans() -> TestResult {
    assert_eq!(serde_json::to_value(LeasePrewarm::Prewarm)?, json!(true));
    assert_eq!(serde_json::to_value(LeasePrewarm::Regular)?, json!(false));
    assert_eq!(
        serde_json::from_value::<LeasePrewarm>(json!(true))?,
        LeasePrewarm::Prewarm
    );
    assert_eq!(serde_json::to_value(LeasePersistence::Persistent)?, json!(true));
    assert_eq!(serde_json::to_value(LeasePersistence::Ephemeral)?, json!(false));
    assert_eq!(
        serde_json::from_value::<LeasePersistence>(json!(false))?,
        LeasePersistence::Ephemeral
    );
    assert_eq!(LeasePrewarm::default(), LeasePrewarm::Regular);
    assert_eq!(LeasePersistence::default(), LeasePersistence::Persistent);
    Ok(())
}

#[test]
fn a_request_with_a_bad_value_is_rejected() -> TestResult {
    let mut wire = minimal_request();
    wire["idempotency_key"] = json!("");
    let error = serde_json::from_value::<LeaseRequest>(wire)
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "lease request key must not be empty");

    let mut wire = minimal_request();
    wire["tags"] = json!({ "a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6" });
    let error = serde_json::from_value::<LeaseRequest>(wire)
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "sandbox supports at most 5 tags, got 6");
    assert_eq!(error.classify(), Data);

    let mut wire = minimal_request();
    wire["name"] = json!("");
    let error = serde_json::from_value::<LeaseRequest>(wire)
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "lease name must not be empty");
    assert_eq!(error.classify(), Data);

    let mut wire = minimal_request();
    wire.as_object_mut().ok_or("object")?.remove("ttl_seconds");
    let error = serde_json::from_value::<LeaseRequest>(wire)
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "missing field `ttl_seconds`");
    Ok(())
}

#[test]
fn a_dispatch_payload_round_trips_with_and_without_optional_fields() -> TestResult {
    let bare = canonical::<ProviderDispatchPayload>(json!({
        "lease_id": "lease-1",
        "request": defaulted_request(),
    }))?;
    assert_eq!(bare.correlation_id, None);
    assert!(bare.resolved_drives.is_empty());
    assert_eq!(bare.target, None);

    let full = canonical::<ProviderDispatchPayload>(json!({
        "lease_id": "lease-1",
        "request": full_request()?,
        "correlation_id": "corr-1",
        "resolved_drives": [{
            "id": { "tenant": "acme", "name": "scratch" },
            "tenant": "acme",
            "size_gib": 100,
            "created_at": time(1),
            "attached_lease": null,
            "bucket": "drive-bucket",
        }],
        "target": { "worker_id": "w1", "attempt_id": "a1", "epoch": 3 },
    }))?;
    assert_eq!(full.resolved_drives.len(), 1);
    assert_eq!(full.correlation_id.as_ref().map(CorrelationId::as_str), Some("corr-1"));
    Ok(())
}

fn minimal_record() -> Value {
    json!({
        "lease_id": "lease-1",
        "tenant": "acme",
        "provider": "mock",
        "status": "running",
        "created_at": time(1),
        "deadline": time(2),
        "spec": spec(),
    })
}

#[test]
fn a_minimal_record_gets_the_documented_defaults() -> TestResult {
    let mut expected = minimal_record();
    expected["sandbox_id"] = json!("");
    expected["cpu_seconds_consumed"] = json!(0);
    expected["wall_seconds_consumed"] = json!(0);
    expected["tags"] = json!({});
    let record = normalized::<LeaseRecord>(minimal_record(), expected)?;
    assert_eq!(record.sandbox_id, "");
    assert_eq!(record.worker_id, None);
    assert_eq!(record.dormancy, None);
    assert_eq!(record.paused_remaining_seconds, None);
    Ok(())
}

#[test]
fn a_full_record_round_trips_exactly() -> TestResult {
    let wire = json!({
        "lease_id": "lease-1",
        "tenant": "acme",
        "provider": "daytona",
        "status": "paused",
        "created_at": time(1),
        "deadline": time(2),
        "spec": spec(),
        "sandbox_id": "sbx-1",
        "cpu_seconds_consumed": 4,
        "wall_seconds_consumed": 9,
        "correlation_id": "corr-1",
        "tags": { "team": "core" },
        "name": "dev",
        "worker_id": "w1",
        "dispatch_attempt": {
            "id": "a1",
            "epoch": 2,
            "idempotency_key": "key-1",
            "accounting": "tenant",
        },
        "quota_reservation": {
            "tenant": "acme",
            "attempt_id": "a1",
            "created_at": time(1),
            "cpu_seconds": 60,
            "wall_seconds": 120,
            "accounting": "tenant",
        },
        "epoch": 2,
        "last_activity_at": time(3),
        "paused_remaining_seconds": 30,
        "freeze_in_flight": { "thaw": false, "started_at": time(4) },
        "auto_paused_at": time(5),
        "idle_pause_retry": { "failures": 1, "retry_at": time(6) },
        "restored_from": format!("sha256:{}", "a".repeat(64)),
        "entity_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "dormancy": {
            "level": "memory",
            "generation": 1,
            "suspended_at": time(7),
            "run_remaining_seconds": 300,
        },
    });
    let record = canonical::<LeaseRecord>(wire)?;
    assert_eq!(record.sandbox_id, "sbx-1");
    assert_eq!(record.paused_remaining_seconds, Some(30));
    Ok(())
}
