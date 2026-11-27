use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn a_new_lease_mints_epoch_one_and_stamps_it_only_for_a_worker_that_supports_it() -> TestResult {
    let (payload, record) = dispatch_to_worker_with_epoch_support(true).await?;
    assert_eq!(payload["target"]["epoch"], 1);
    assert_eq!(record.epoch, Some(trogon_sandbox_contract::LeaseEpoch::FIRST));
    assert_eq!(
        record.dispatch_attempt.as_ref().ok_or("dispatch attempt")?.epoch,
        Some(trogon_sandbox_contract::LeaseEpoch::FIRST)
    );
    Ok(())
}

#[tokio::test]
async fn a_worker_without_the_capability_gets_no_epoch_on_the_wire() -> TestResult {
    let (payload, record) = dispatch_to_worker_with_epoch_support(false).await?;
    assert!(payload["target"].get("epoch").is_none());
    assert_eq!(record.epoch, Some(trogon_sandbox_contract::LeaseEpoch::FIRST));
    assert_eq!(record.dispatch_attempt.as_ref().ok_or("dispatch attempt")?.epoch, None);
    Ok(())
}

#[test]
fn records_and_heartbeats_written_before_epochs_still_deserialize() -> TestResult {
    let mut heartbeat = serde_json::to_value(trogon_sandbox_contract::WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: trogon_sandbox_contract::WorkerId::new("w")?,
        provider: ProviderKind::Fly,
        dispatch_protocol: trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1,
        native_volume_bindings: vec![],
        capacity: trogon_sandbox_contract::WorkerCapacity::default(),
        emitted_at: SystemTime::now(),
        cached_digests: vec![],
        profile: crate::default_capability_profile::default_capability_profile(),
        warm_pool_population: Default::default(),
    })?;
    heartbeat
        .as_object_mut()
        .ok_or("a heartbeat serializes as an object")?
        .remove("supports_lease_epoch");
    let parsed: trogon_sandbox_contract::WorkerHeartbeat = serde_json::from_value(heartbeat)?;
    assert!(!parsed.supports_lease_epoch.is_supported());
    let lease: CoreProviderLease =
        serde_json::from_value(serde_json::json!({"sandbox_id": "s", "evidence_subject": "e"}))?;
    assert_eq!(lease.epoch, None);
    Ok(())
}

#[tokio::test]
async fn drive_creation_refuses_a_drive_id_owned_by_another_tenant() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let response = core
        .handle_create_drive(CreateDriveRequest {
            tenant: TenantId::new("requester")?,
            drive_id: make_drive_id(&TenantId::new("other")?, "data")?,
            size_gib: Some(10),
            storage: trogon_sandbox_contract::DriveStorage::S3 {
                bucket: "test-bucket".into(),
            },
        })
        .await;
    assert!(matches!(response, CreateDriveResponse::Denied(_)), "{response:?}");
    Ok(())
}
