use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn on_mode_end_to_end_against_the_nats_entity_store() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = async_nats::jetstream::new(server.client.clone());
    let store = Arc::new(crate::entity_store::NatsKvSandboxEntityStore::new(js).await?);
    let core = with_mode(
        make_core_with_purge_named_response(PurgeNamedResponse::NotFound)?,
        store.clone(),
        SandboxEntityMode::On,
    );

    let first = granted(core.handle_lease_request(named("k1")?).await)?;
    let id = first.entity_id.clone().ok_or("entity id")?;
    release(&core, &first.lease_id).await?;
    assert_eq!(core.resolve_named_lease(&tenant()?, "workspace").await, Ok(None));

    let second = granted(core.handle_lease_request(named("k2")?).await)?;
    assert_eq!(second.entity_id, Some(id.clone()));
    let GetEntityResponse::Found { row } = core
        .handle_get_entity(GetEntityRequest {
            tenant: tenant()?,
            entity: EntityRef::Name("workspace".into()),
        })
        .await
    else {
        return Err("entity not found".into());
    };
    assert_eq!(row.entity_id, id);
    assert_eq!(row.generation, 2);
    assert_eq!(row.current_lease, Some(second.lease_id.clone()));

    let ListEntitiesResponse::Listed { rows, next_cursor } = core
        .handle_list_entities(ListEntitiesRequest {
            tenant: tenant()?,
            limit: None,
            cursor: None,
        })
        .await
    else {
        return Err("list denied".into());
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(next_cursor, None);

    assert_eq!(
        core.handle_delete_entity(DeleteEntityRequest {
            tenant: tenant()?,
            entity: EntityRef::Id(id.clone()),
        })
        .await,
        DeleteEntityResponse::Deleted
    );
    assert_eq!(
        core.handle_get_entity(GetEntityRequest {
            tenant: tenant()?,
            entity: EntityRef::Id(id),
        })
        .await,
        GetEntityResponse::NotFound
    );
    assert!(store.get_by_name(&tenant()?, "workspace").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn the_jetstream_sink_publishes_events_per_entity() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = async_nats::jetstream::new(server.client.clone());
    let sink = crate::entity_shadow::JetStreamEntityEventSink::new(js.clone(), 1).await?;
    let shadow = EntityShadow::new(
        SandboxEntityMode::Shadow,
        Arc::new(InMemorySandboxEntityStore::new()),
        Arc::new(sink),
        Duration::from_secs(1),
    );
    let core = make_core(mock_provider_response()?)?.with_entity_shadow(Arc::new(shadow));
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    let id = grant.entity_id.ok_or("entity id")?;
    let subject = crate::entity_shadow::entity_event_subject(&TenantId::new("test-tenant")?, &id);
    let mut stream = js.get_stream("sandbox-entity-events").await?;
    let mut found = 0;
    for _ in 0..50 {
        found = stream.info().await?.state.messages;
        if found >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(found >= 2, "Created and RunStarted, got {found}");
    let raw = stream.get_last_raw_message_by_subject(&subject).await?;
    let event: EntityLifecycleEvent = serde_json::from_slice(&raw.payload)?;
    assert_eq!(event.entity_id(), &id);
    Ok(())
}
