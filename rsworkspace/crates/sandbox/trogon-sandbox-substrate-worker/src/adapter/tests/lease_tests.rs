use async_nats::jetstream::stream;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_actor_agent::{BIND_PATH, READYZ_PATH};
use trogon_sandbox_protocol::lifecycle_subject;

use super::super::tests_support::{BrokenIndex, FakeControl, TestResult, config, lease_request, provider_over};
use super::*;
use crate::agent::ActorTarget;
use crate::agent::tests::{agent_router, serve};
use crate::ate_client::AteError;

/// An agent that is ready and accepts every bind.
fn healthy_agent() -> Router {
    Router::new()
        .route(READYZ_PATH, get(|| async { StatusCode::OK }))
        .route(BIND_PATH, post(|| async { StatusCode::OK }))
}

/// An agent that is ready but refuses every bind.
fn refusing_agent() -> Router {
    Router::new()
        .route(READYZ_PATH, get(|| async { StatusCode::OK }))
        .route(BIND_PATH, post(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
}

/// A provider whose router fronts `agent` for the actor `lease` names.
async fn provider_serving(
    control: Arc<FakeControl>,
    index: Arc<dyn crate::actor_index::ActorIndex>,
    lease: &ActiveLease,
    agent: Router,
) -> Result<SubstrateProvider, Box<dyn Error>> {
    control.set_uid(TEST_UID);
    let target = ActorTarget::new(lease.atespace.clone(), lease.actor.clone(), 8110);
    let fronted = serve(target, agent).await?;
    let mut config = config()?;
    config.router_endpoint = agent_router(&fronted);
    Ok(provider_over(config, control, index)?)
}

fn ownership(error: SubstrateError, needle: &str) -> TestResult {
    match error {
        SubstrateError::Ownership(message) => {
            assert!(message.contains(needle), "{message:?} does not mention {needle:?}");
            Ok(())
        }
        other => Err(format!("expected Ownership, got {other:?}").into()),
    }
}

#[tokio::test]
async fn an_anonymous_lease_is_granted_bound_and_then_cancelled() -> TestResult {
    let control = Arc::new(FakeControl::running().with_transitions());
    let lease_id = LeaseId::new("01J0");
    let lease = anonymous(&lease_id, false, None)?;
    let provider = provider_serving(
        control.clone(),
        Arc::new(MemoryActorIndex::default()),
        &lease,
        healthy_agent(),
    )
    .await?;

    let granted = Provider::lease(&provider, &lease_request(None, false)?, &lease_id).await?;

    assert_eq!(granted.sandbox_id.as_str(), lease.sandbox_id());
    assert_eq!(granted.worker_id.as_ref(), Some(provider.worker_id()));
    assert_eq!(provider.current_lease_count().await, 1);
    let entry = indexed(&provider, &lease).await?.ok_or("no claim")?;
    assert_eq!(entry.owner.and_then(|owner| owner.actor_uid), Some(test_uid()?));
    assert!(provider.agent_for(&lease_id).await.is_some());

    // The same lease asked for again is the same grant, without a second actor.
    let again = Provider::lease(&provider, &lease_request(None, false)?, &lease_id).await?;
    assert_eq!(again.sandbox_id, granted.sandbox_id);
    assert_eq!(control.resume_count(), 1);

    Provider::cancel(&provider, &lease_id).await?;
    assert_eq!(control.delete_count(), 1);
    assert!(indexed(&provider, &lease).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_named_lease_resumes_the_retained_actor_it_left_behind() -> TestResult {
    let control = Arc::new(FakeControl::running().with_transitions());
    let first = LeaseId::new("01J1");
    let lease = named("work", true)?;
    let provider = provider_serving(
        control.clone(),
        Arc::new(MemoryActorIndex::default()),
        &lease,
        healthy_agent(),
    )
    .await?;
    let request = lease_request(Some("work"), true)?;

    Provider::lease(&provider, &request, &first).await?;
    Provider::cancel(&provider, &first).await?;
    assert!(indexed(&provider, &lease).await?.ok_or("no claim")?.retained);

    let second = LeaseId::new("01J2");
    let granted = Provider::lease(&provider, &request, &second).await?;

    assert_eq!(granted.sandbox_id.as_str(), lease.sandbox_id());
    assert_eq!(control.delete_count(), 0);
    assert_eq!(control.resume_count(), 2);
    let entry = indexed(&provider, &lease).await?.ok_or("no claim")?;
    assert_eq!(entry.lease_id, second);
    assert!(!entry.retained);
    Ok(())
}

#[tokio::test]
async fn a_lease_that_cannot_bind_leaves_cleanup_for_the_authority() -> TestResult {
    let control = Arc::new(FakeControl::running().with_transitions());
    let lease_id = LeaseId::new("01J3");
    let lease = anonymous(&lease_id, false, None)?;
    let provider = provider_serving(
        control.clone(),
        Arc::new(MemoryActorIndex::default()),
        &lease,
        refusing_agent(),
    )
    .await?;

    let error = provider
        .lease_inner(&lease_request(None, false)?, &lease_id)
        .await
        .err()
        .ok_or("granted")?;

    assert!(matches!(error, SubstrateError::Agent(_)), "{error:?}");
    assert_eq!(AuthorityHost::held_leases(&provider).await?.len(), 1);
    assert!(provider.agent_for(&lease_id).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_lease_publishes_its_lifecycle_to_the_evidence_stream() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let lease_id = LeaseId::new("01J4");
    let mut stream = js
        .create_stream(stream::Config {
            name: "LIFECYCLE".to_string(),
            subjects: vec![lifecycle_subject(&lease_id)],
            ..Default::default()
        })
        .await?;
    let control = Arc::new(FakeControl::running().with_transitions());
    let lease = anonymous(&lease_id, false, None)?;
    let provider = provider_serving(control, Arc::new(MemoryActorIndex::default()), &lease, healthy_agent())
        .await?
        .with_lifecycle_stream(js);

    Provider::lease(&provider, &lease_request(None, false)?, &lease_id).await?;

    assert!(stream.info().await?.state.messages > 0);
    Ok(())
}

#[tokio::test]
async fn a_lease_the_actor_cannot_serve_is_refused_before_any_actor_exists() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01ARZ");

    let mut parts = super::super::tests_support::spec_parts()?;
    parts.memory_mib = std::num::NonZeroU32::new(u32::MAX).ok_or("memory")?;
    let mut oversized = lease_request(None, false)?;
    oversized.spec = trogon_sandbox_contract::SandboxSpec::new(parts)?;
    let error = provider
        .lease_inner(&oversized, &lease_id)
        .await
        .err()
        .ok_or("granted")?;
    assert!(
        matches!(error, SubstrateError::InvalidSpec(ref message) if message.contains("largest")),
        "{error:?}"
    );

    let mut unsupported = lease_request(None, false)?;
    unsupported.isolation_tier = Some(trogon_sandbox_contract::IsolationTier::FullVm);
    let error = provider
        .lease_inner(&unsupported, &lease_id)
        .await
        .err()
        .ok_or("granted")?;
    assert!(matches!(error, SubstrateError::InvalidSpec(_)), "{error:?}");

    assert_eq!(control.resume_count() + control.get_count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_tenant_that_is_not_a_dns_label_never_reaches_the_control_plane() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    let mut request = lease_request(None, false)?;
    request.tenant = TenantId::new("Tenant_A")?;

    let error = provider
        .lease_inner(&request, &LeaseId::new("01ARZ"))
        .await
        .err()
        .ok_or("granted")?;

    assert!(matches!(error, SubstrateError::Naming(_)), "{error:?}");
    assert_eq!(control.resume_count(), 0);
    Ok(())
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(error_string_comparison, reason = "asserts the refusal names the offending field")
)]
#[tokio::test]
async fn a_lease_the_provider_cannot_honor_is_refused_by_the_provider_api() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let mut parts = super::super::tests_support::spec_parts()?;
    parts.vcpu_millis = Some(4000);
    let mut request = lease_request(None, false)?;
    request.spec = trogon_sandbox_contract::SandboxSpec::new(parts)?;

    let error = Provider::lease(&provider, &request, &LeaseId::new("01ARZ"))
        .await
        .err()
        .ok_or("granted")?;

    assert!(error.to_string().contains("vcpu_millis"));
    assert_eq!(Provider::kind(&provider), ProviderKind::Substrate);
    Ok(())
}

#[tokio::test]
async fn an_actor_held_by_another_assignment_is_not_leased() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J5");
    let lease = anonymous(&lease_id, false, None)?;
    provider
        .index()
        .put(&claim(&lease.atespace, &lease.actor, &LeaseId::new("someone-else")))
        .await?;

    let error = provider
        .lease_inner(&lease_request(None, false)?, &lease_id)
        .await
        .err()
        .ok_or("granted")?;

    ownership(error, "another or unconfirmed assignment")?;
    assert_eq!(control.resume_count(), 0);
    Ok(())
}

async fn retained(control: Arc<FakeControl>) -> Result<(SubstrateProvider, ActiveLease), Box<dyn Error>> {
    let provider = provider(control)?;
    let lease = named("work", true)?;
    let mut entry = owned_entry(&provider, &LeaseId::new("01J6"), &lease)?;
    entry.retained = true;
    entry.named = true;
    provider.index().put(&entry).await?;
    Ok((provider, lease))
}

#[tokio::test]
async fn a_retained_actor_that_is_not_suspended_is_not_reused() -> TestResult {
    let (provider, _) = retained(Arc::new(FakeControl::running())).await?;
    let error = provider
        .lease_inner(&lease_request(Some("work"), true)?, &LeaseId::new("01J7"))
        .await
        .err()
        .ok_or("granted")?;
    ownership(error, "not positively suspended")
}

#[tokio::test]
async fn a_retained_actor_that_disappeared_is_not_reused() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    let (provider, _) = retained(control.clone()).await?;
    control.fail_get_with(AteError::NotFound { message: "gone".into() });
    let error = provider
        .lease_inner(&lease_request(Some("work"), true)?, &LeaseId::new("01J8"))
        .await
        .err()
        .ok_or("granted")?;
    ownership(error, "retained actor is absent")
}

#[tokio::test]
async fn a_retained_actor_running_another_image_is_not_reused() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    control.set_existing_template("some-other-template");
    let (provider, _) = retained(control).await?;
    let error = provider
        .lease_inner(&lease_request(Some("work"), true)?, &LeaseId::new("01J9"))
        .await
        .err()
        .ok_or("granted")?;
    assert!(matches!(error, SubstrateError::TemplateMismatch { .. }), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn a_lease_fails_while_the_ownership_store_is_down() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.set_uid(TEST_UID);
    let provider = super::super::tests_support::provider_with(control.clone(), Arc::new(BrokenIndex))?;
    let error = provider
        .lease_inner(&lease_request(None, false)?, &LeaseId::new("01JA"))
        .await
        .err()
        .ok_or("granted")?;
    ownership(error, "storage is down")?;
    assert_eq!(control.resume_count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_control_plane_that_rejects_the_atespace_fails_the_lease() -> TestResult {
    // The fake accepts every atespace, so the failure comes from the allocation instead.
    let control = Arc::new(FakeControl::running());
    control.fail_create_with(AteError::PermissionDenied { message: "no".into() });
    let provider = provider(control)?;
    let error = provider
        .lease_inner(&lease_request(None, false)?, &LeaseId::new("01JB"))
        .await
        .err()
        .ok_or("granted")?;
    assert!(
        matches!(error, SubstrateError::Ate(AteError::PermissionDenied { .. })),
        "{error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_atespace_the_control_plane_refuses_fails_the_lease() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_atespace_with(AteError::PermissionDenied { message: "no".into() });
    let provider = provider(control.clone())?;
    let error = provider
        .lease_inner(&lease_request(None, false)?, &LeaseId::new("01JC"))
        .await
        .err()
        .ok_or("granted")?;
    assert!(
        matches!(error, SubstrateError::Ate(AteError::PermissionDenied { .. })),
        "{error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_atespace_that_already_exists_is_not_a_failure() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_atespace_with(AteError::AlreadyExists {
        message: "there".into(),
    });
    let provider = provider(control)?;
    provider.ensure_atespace(&atespace()?).await?;
    Ok(())
}

#[tokio::test]
async fn a_microvm_actor_below_its_memory_floor_is_refused() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.set_uid(TEST_UID);
    let mut config = config()?;
    config.sandbox_class = crate::config::SandboxClass::MicroVm;
    config.size_classes = "tiny=64,small=2048".parse()?;
    let provider = provider_over(config, control.clone(), Arc::new(MemoryActorIndex::default()))?;
    let mut parts = super::super::tests_support::spec_parts()?;
    parts.memory_mib = std::num::NonZeroU32::new(64).ok_or("memory")?;
    let mut request = lease_request(None, false)?;
    request.spec = trogon_sandbox_contract::SandboxSpec::new(parts)?;

    let error = provider
        .lease_inner(&request, &LeaseId::new("01JD"))
        .await
        .err()
        .ok_or("granted")?;

    assert!(matches!(error, SubstrateError::InvalidSpec(_)), "{error:?}");
    assert_eq!(control.resume_count(), 0);
    Ok(())
}

#[tokio::test]
async fn an_actor_reached_through_a_router_identity_uses_it() -> TestResult {
    let mut config = config()?;
    config.router_tls = Some(crate::router_tls::RouterTlsConfig::new(
        "/nonexistent/ca.pem",
        "/nonexistent/client.pem",
        "/nonexistent/client.key",
        "localhost",
    )?);
    let provider = provider_over(
        config,
        Arc::new(FakeControl::running()),
        Arc::new(MemoryActorIndex::default()),
    )?;
    let lease = anonymous(&LeaseId::new("01JE"), false, None)?;
    let agent = provider.actor_agent(ActorTarget::new(lease.atespace, lease.actor, 8110));
    let request: trogon_sandbox_contract::ReadFileRequest =
        serde_json::from_str(r#"{"lease_id":"01JE","path":"/workspace/x"}"#)?;

    let error = agent
        .read_file(&request)
        .await
        .err()
        .ok_or("the identity does not exist")?;

    assert!(
        matches!(error, crate::agent::AgentError::Unreachable { .. }),
        "{error:?}"
    );
    Ok(())
}
