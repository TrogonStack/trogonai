mod support;

use std::time::Duration;

use axum::http::Method;
use serde_json::json;
use support::*;
use trogon_sandbox_proxmox_worker::{ProxmoxApi, ProxmoxApiError, VmId};

const TEMPLATE: u32 = 100;

fn ids() -> Boxed<(VmId, VmId)> {
    Ok((VmId::try_from(TEMPLATE)?, VmId::try_from(VM)?))
}

fn task_polls(fake: &Fake, kind: &str) -> usize {
    fake.calls()
        .iter()
        .filter(|call| call.path.contains("/tasks/") && call.path.contains(kind))
        .count()
}

async fn api(fake: &Fake) -> Boxed<ProxmoxApi> {
    fake.api(Duration::from_secs(1))
}

#[tokio::test]
async fn a_linked_clone_waits_for_its_clone_task() -> TestResult {
    let fake = Fake::start().await?;
    let (template, vm) = ids()?;
    api(&fake).await?.linked_clone(template, vm, "linked").await?;
    assert!(fake.clone_exists());
    assert_eq!(fake.with(|host| host.clone_name.clone()).as_deref(), Some("linked"));
    let clone = fake
        .calls()
        .into_iter()
        .find(|call| call.path.ends_with(&format!("/qemu/{TEMPLATE}/clone")))
        .ok_or("no clone request")?;
    assert_eq!(clone.body["newid"], VM);
    assert_eq!(clone.body["full"], 0);
    assert_eq!(task_polls(&fake, "clone"), 1);
    Ok(())
}

#[tokio::test]
async fn destroy_waits_for_the_delete_task() -> TestResult {
    let fake = Fake::start().await?;
    let (template, vm) = ids()?;
    let api = api(&fake).await?;
    api.linked_clone(template, vm, "doomed").await?;
    api.destroy(vm).await?;
    assert!(!fake.clone_exists());
    assert_eq!(fake.count(&Method::DELETE, &format!("/qemu/{VM}")), 1);
    assert_eq!(task_polls(&fake, "delete"), 1);
    Ok(())
}

#[tokio::test]
async fn a_destroy_whose_task_never_stops_times_out() -> TestResult {
    let fake = Fake::start_with(Host {
        task_running: true,
        ..Default::default()
    })
    .await?;
    let api = fake.api(Duration::from_millis(300))?;
    let result = api.destroy(VmId::try_from(VM)?).await;
    assert!(matches!(result, Err(ProxmoxApiError::TaskTimedOut)), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn a_snapshot_keeps_the_vm_memory_and_waits_for_its_task() -> TestResult {
    let fake = Fake::start().await?;
    api(&fake).await?.snapshot(VmId::try_from(VM)?, "before").await?;
    let snapshot = fake
        .calls()
        .into_iter()
        .find(|call| call.method == Method::POST && call.path.ends_with(&format!("/qemu/{VM}/snapshot")))
        .ok_or("no snapshot request")?;
    assert_eq!(snapshot.body, json!({"snapname": "before", "vmstate": 1}));
    assert_eq!(fake.with(|host| host.snapshots.clone()), vec!["before".to_owned()]);
    assert_eq!(task_polls(&fake, "snapshot"), 1);
    Ok(())
}

#[tokio::test]
async fn a_rollback_returns_to_the_named_snapshot() -> TestResult {
    let fake = Fake::start().await?;
    let api = api(&fake).await?;
    let vm = VmId::try_from(VM)?;
    api.snapshot(vm, "before").await?;
    api.rollback(vm, "before").await?;
    assert_eq!(
        fake.count(&Method::POST, &format!("/qemu/{VM}/snapshot/before/rollback")),
        1
    );
    assert_eq!(task_polls(&fake, "rollback"), 1);
    Ok(())
}

#[tokio::test]
async fn a_rollback_to_an_unknown_snapshot_is_refused_by_the_host() -> TestResult {
    let fake = Fake::start().await?;
    let result = api(&fake).await?.rollback(VmId::try_from(VM)?, "never-taken").await;
    assert!(
        matches!(result, Err(ProxmoxApiError::Status(status)) if status.as_u16() == 500),
        "{result:?}"
    );
    assert_eq!(task_polls(&fake, "rollback"), 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_task_fails_the_operation_that_waited_on_it() -> TestResult {
    let fake = Fake::start_with(Host {
        failed_tasks: true,
        snapshots: vec!["before".to_owned()],
        ..Default::default()
    })
    .await?;
    let api = api(&fake).await?;
    let (template, vm) = ids()?;
    assert!(matches!(
        api.linked_clone(template, vm, "linked").await,
        Err(ProxmoxApiError::TaskFailed)
    ));
    assert!(matches!(
        api.snapshot(vm, "after").await,
        Err(ProxmoxApiError::TaskFailed)
    ));
    assert!(matches!(
        api.rollback(vm, "before").await,
        Err(ProxmoxApiError::TaskFailed)
    ));
    assert!(matches!(api.destroy(vm).await, Err(ProxmoxApiError::TaskFailed)));
    Ok(())
}
