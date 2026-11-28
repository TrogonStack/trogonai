use super::super::tests_support::{BrokenIndex, FakeControl, TestResult, WarmTemplates};
use super::*;
use crate::ate_client::{Control, ResourceRevision};
use crate::template::{TemplateIndex, TemplatePhase, TemplateRecord};

fn revision() -> Result<ResourceRevision, AteError> {
    ResourceRevision::from_metadata(Some(&proto::ResourceMetadata {
        atespace: "tenant-a".to_string(),
        name: "actor-1".to_string(),
        uid: "uid-1".to_string(),
        version: 1,
        ..Default::default()
    }))
}

#[tokio::test]
async fn the_fake_control_lists_the_actors_it_was_told_about() -> TestResult {
    let control = FakeControl::running();
    control.set_listed(vec!["one".to_string(), "two".to_string()]);
    let actors = control.list_actors("tenant-a").await?;
    assert_eq!(actors.len(), 2);
    Ok(())
}

#[tokio::test]
async fn the_fake_control_accepts_template_traffic() -> TestResult {
    let control = FakeControl::running();
    control.create_actor_template(proto::ActorTemplate::default()).await?;
    control.delete_actor_template(revision()?).await?;
    Ok(())
}

#[tokio::test]
async fn warm_templates_hold_nothing_and_accept_everything() -> TestResult {
    let record = TemplateRecord {
        atespace: "tenant-a".to_string(),
        name: "template".to_string(),
        phase: TemplatePhase::Ready,
        created_at: 0,
        message: None,
    };
    assert!(WarmTemplates.get("key").await?.is_none());
    assert!(!WarmTemplates.create("key", &record).await?);
    WarmTemplates.put("key", &record).await?;
    WarmTemplates.delete("key").await?;
    Ok(())
}

#[tokio::test]
async fn a_broken_index_fails_every_operation() -> TestResult {
    let lease_id = LeaseId::new("01LB");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = claim(&lease.atespace, &lease.actor, &lease_id);
    assert!(BrokenIndex.put(&entry).await.is_err());
    assert!(BrokenIndex.get("a", "b").await.is_err());
    assert!(BrokenIndex.delete("a", "b").await.is_err());
    assert!(BrokenIndex.all().await.is_err());
    assert!(BrokenIndex.compare_exchange(None, Some(&entry)).await.is_err());
    Ok(())
}

#[test]
fn an_unparseable_minimum_override_refuses_the_provider() -> TestResult {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(
        trogon_sandbox_actor_agent::MIN_AGENT_PROTOCOL_VERSION_ENV,
        "not-a-number",
    );
    let result = super::super::tests_support::provider_in(
        super::super::tests_support::config()?,
        Arc::new(FakeControl::running()),
        Arc::new(crate::actor_index::memory::MemoryActorIndex::default()),
        &env,
    );
    assert!(
        matches!(result, Err(SubstrateError::MinProtocolVersion(_))),
        "{:?}",
        result.err()
    );
    Ok(())
}
