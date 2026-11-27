use super::*;

async fn fresh_nats_operation_store(
    server: &crate::test_support::LocalNats,
) -> anyhow::Result<Arc<dyn crate::operations::OperationStore>> {
    let client = server.fresh_client().await;
    Ok(Arc::new(
        crate::operations::NatsKvOperationStore::new(
            async_nats::jetstream::new(client),
            "operation-cancellation-proof",
            1,
        )
        .await?,
    ))
}

#[tokio::test]
async fn operation_cancel_precancel_survives_real_nats_client_and_store_reconstruction() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let bus = LaunchBus::new(false);
    let first_store = fresh_nats_operation_store(&server).await?;
    let first = durable_core(bus.clone(), first_store.clone(), "broker-a")?;
    let request = create_with(ProviderKind::Mock, "real-nats-precancel")?;
    assert_eq!(
        first
            .handle_cancel_operation(cancel(&request.tenant, "real-nats-precancel")?)
            .await,
        CancelOperationResponse::Pending
    );
    drop(first);
    drop(first_store);
    let reopened_store = fresh_nats_operation_store(&server).await?;
    let second = durable_core(bus.clone(), reopened_store.clone(), "broker-b")?;
    assert!(matches!(
        second.handle_lease_request(request.clone()).await,
        LeaseResponse::Denied(LeaseError::OperationCanceled { .. })
    ));
    assert_eq!(bus.launches()?, 0);
    let operations = crate::operations::Operations::new(reopened_store, "reader");
    let stored = operations
        .get(&request.tenant, &op_id("real-nats-precancel")?)
        .await?
        .ok_or("the operation is missing")?
        .record;
    assert_eq!(stored.state, OperationState::Canceled);
    assert!(stored.cancellation.is_some());
    assert!(operations.list_active().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn operation_cancel_compensation_recovers_automatically_from_reopened_real_nats_journal() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let bus = CancelableLaunchBus::new();
    bus.release.notify_one();
    let first_store = fresh_nats_operation_store(&server).await?;
    let first = durable_core(bus.clone(), first_store.clone(), "broker-a")?;
    let request = create_with(ProviderKind::E2b, "real-nats-compensation")?;
    let tenant = request.tenant.clone();
    let LeaseResponse::Granted(grant) = first.handle_lease_request(request).await else {
        return Err("owned create completed".into());
    };
    bus.cleanup_fails.store(true, Ordering::SeqCst);
    assert_eq!(
        first
            .handle_cancel_operation(compensate(&tenant, "real-nats-compensation")?)
            .await,
        CancelOperationResponse::Pending
    );
    let GetOperationResponse::Found { operation: pending } = first
        .handle_get_operation(GetOperationRequest {
            tenant: tenant.clone(),
            operation_id: op_id("real-nats-compensation")?,
        })
        .await
    else {
        return Err("pending state remains visible".into());
    };
    assert_eq!(pending.state, OperationState::Succeeded);
    assert_eq!(
        pending.compensation,
        Some(trogon_sandbox_contract::OperationCompensationState::Pending)
    );
    let reopened_store = fresh_nats_operation_store(&server).await?;
    let second = restarted(&first, bus.clone(), reopened_store.clone())?;
    drop(first);
    drop(first_store);
    bus.cleanup_fails.store(false, Ordering::SeqCst);
    assert_eq!(
        second.recover_operations().await,
        1,
        "durable cleanup markers must be scanned without another cancellation request"
    );
    let GetOperationResponse::Found { operation: completed } = second
        .handle_get_operation(GetOperationRequest {
            tenant,
            operation_id: op_id("real-nats-compensation")?,
        })
        .await
    else {
        return Err("completed compensation is readable".into());
    };
    assert_eq!(completed.state, OperationState::Succeeded);
    assert_eq!(
        completed.compensation,
        Some(trogon_sandbox_contract::OperationCompensationState::Cleaned)
    );
    assert!(
        matches!(completed.result, Some(OperationResult::Granted(ref stored)) if stored.lease_id == grant.lease_id && stored.created.is_created())
    );
    {
        let cleaned = locked(&bus.owned_cleaned)?;
        assert_eq!(cleaned.len(), 2);
        assert!(cleaned.iter().all(|request| request == &cleaned[0]));
        assert_eq!(cleaned[0].sandbox_id.as_str(), grant.sandbox_id);
    }
    assert!(
        crate::operations::Operations::new(reopened_store, "reader")
            .list_active()
            .await?
            .is_empty()
    );
    Ok(())
}
