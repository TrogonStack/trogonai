use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    LeaseCreation, LeaseError, LeaseGranted, LeaseId, LeaseResponse, NetworkPolicy, NetworkPolicyUpdateRequest,
    OperationId, ProviderKind, SandboxEntityId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn time(secs: u64) -> Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn roundtrip<T>(value: &T, expected: &Value) -> TestResult
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(&serde_json::to_value(value)?, expected);
    assert_eq!(&serde_json::from_value::<T>(expected.clone())?, value);
    Ok(())
}

fn granted() -> Result<LeaseGranted, Box<dyn std::error::Error>> {
    Ok(LeaseGranted {
        lease_id: LeaseId::new("lease-1"),
        provider: ProviderKind::LocalDocker,
        sandbox_id: "box-1".to_owned(),
        evidence_subject: "evidence.lease-1".to_owned(),
        deadline: at(10),
        created: LeaseCreation::Created,
        capabilities: vec!["files".to_owned()],
        entity_id: Some(SandboxEntityId::new("01ARZ3NDEKTSV4RRFFQ69G5FAV")?),
        operation_id: Some(OperationId::new("op-1")?),
    })
}

fn granted_json() -> Value {
    json!({
        "lease_id": "lease-1",
        "provider": "local_docker",
        "sandbox_id": "box-1",
        "evidence_subject": "evidence.lease-1",
        "deadline": time(10),
        "created": true,
        "capabilities": ["files"],
        "entity_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "operation_id": "op-1",
    })
}

#[test]
fn lease_granted_roundtrips_with_every_field() -> TestResult {
    roundtrip(&granted()?, &granted_json())
}

#[test]
fn lease_granted_defaults_and_omits_optional_fields() -> TestResult {
    let wire = json!({
        "lease_id": "lease-1",
        "provider": "local_docker",
        "sandbox_id": "box-1",
        "evidence_subject": "evidence.lease-1",
        "deadline": time(10),
    });
    let parsed = serde_json::from_value::<LeaseGranted>(wire)?;
    assert_eq!(parsed.created, LeaseCreation::Existing);
    assert!(parsed.capabilities.is_empty());
    assert_eq!(parsed.entity_id, None);
    assert_eq!(parsed.operation_id, None);
    let out = serde_json::to_value(&parsed)?;
    assert_eq!(out["created"], json!(false));
    for key in ["capabilities", "entity_id", "operation_id"] {
        assert!(out.get(key).is_none(), "{key}");
    }
    Ok(())
}

#[test]
fn lease_creation_maps_to_and_from_a_boolean() {
    assert_eq!(LeaseCreation::default(), LeaseCreation::Existing);
    assert!(LeaseCreation::Created.is_created());
    assert!(!LeaseCreation::Existing.is_created());
    assert_eq!(LeaseCreation::from(true), LeaseCreation::Created);
    assert_eq!(LeaseCreation::from(false), LeaseCreation::Existing);
    assert!(bool::from(LeaseCreation::Created));
    assert!(!bool::from(LeaseCreation::Existing));
}

#[test]
fn lease_response_roundtrips_each_outcome() -> TestResult {
    let mut granted_wire = granted_json();
    granted_wire["outcome"] = json!("granted");
    roundtrip(&LeaseResponse::Granted(granted()?), &granted_wire)?;
    roundtrip(
        &LeaseResponse::Denied(LeaseError::NoProviderAvailable),
        &json!({ "outcome": "denied", "kind": "no_provider_available" }),
    )
}

#[test]
fn network_policy_update_roundtrips() -> TestResult {
    roundtrip(
        &NetworkPolicyUpdateRequest {
            lease_id: LeaseId::new("lease-1"),
            network_policy: NetworkPolicy::DenyAll,
        },
        &json!({ "lease_id": "lease-1", "network_policy": "deny_all" }),
    )
}
