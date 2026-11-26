use serde_json::json;
use trogon_sandbox_contract::{
    LeaseEpoch, LeaseId, OwnedLease, WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn request_round_trips() -> TestResult {
    let request = WorkerAuthorityRequest {
        worker_id: WorkerId::new("worker-1")?,
        lease_ids: vec![LeaseId::new("a"), LeaseId::new("b")],
    };
    let value = json!({ "worker_id": "worker-1", "lease_ids": ["a", "b"] });

    assert_eq!(serde_json::to_value(&request)?, value);
    assert_eq!(serde_json::from_value::<WorkerAuthorityRequest>(value)?, request);
    Ok(())
}

#[test]
fn owned_lease_omits_an_absent_epoch() -> TestResult {
    let legacy = OwnedLease {
        lease_id: LeaseId::new("a"),
        epoch: None,
    };
    let current = OwnedLease {
        lease_id: LeaseId::new("b"),
        epoch: Some(LeaseEpoch::FIRST),
    };

    assert_eq!(serde_json::to_value(&legacy)?, json!({ "lease_id": "a" }));
    assert_eq!(
        serde_json::from_value::<OwnedLease>(json!({ "lease_id": "a" }))?,
        legacy
    );
    assert_eq!(serde_json::to_value(&current)?, json!({ "lease_id": "b", "epoch": 1 }));
    assert_eq!(
        serde_json::from_value::<OwnedLease>(json!({ "lease_id": "b", "epoch": 1 }))?,
        current
    );
    Ok(())
}

#[test]
fn owned_lease_rejects_a_zero_epoch() -> TestResult {
    let error = serde_json::from_value::<OwnedLease>(json!({ "lease_id": "a", "epoch": 0 }))
        .err()
        .ok_or("expected error")?;

    assert_eq!(error.classify(), serde_json::error::Category::Data);
    Ok(())
}

#[test]
fn response_defaults_not_assigned_and_round_trips() -> TestResult {
    let sparse: WorkerAuthorityResponse = serde_json::from_value(json!({ "owned": [] }))?;
    assert_eq!(sparse, WorkerAuthorityResponse::default());

    let response = WorkerAuthorityResponse {
        owned: vec![OwnedLease {
            lease_id: LeaseId::new("a"),
            epoch: Some(LeaseEpoch::FIRST),
        }],
        not_assigned: vec![LeaseId::new("b")],
    };
    let value = json!({ "owned": [{ "lease_id": "a", "epoch": 1 }], "not_assigned": ["b"] });

    assert_eq!(serde_json::to_value(&response)?, value);
    assert_eq!(serde_json::from_value::<WorkerAuthorityResponse>(value)?, response);
    Ok(())
}
