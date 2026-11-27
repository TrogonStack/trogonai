use std::time::Duration;

use k8s_openapi::api::batch::v1::{Job, JobCondition, JobStatus};
use tokio::sync::watch;

use super::fake_api_server_tests::FakeApiServer;
use super::fake_reply_tests::FakeReply;
use super::test_objects_tests::job;
use crate::SandboxStatus;
use crate::k8s_ops::K8sOps;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn with_status(name: &str, status: JobStatus) -> Job {
    Job {
        status: Some(status),
        ..job(name)
    }
}

fn condition(type_: &str, message: Option<&str>) -> JobStatus {
    JobStatus {
        conditions: Some(vec![JobCondition {
            type_: type_.to_string(),
            status: "True".to_string(),
            message: message.map(str::to_string),
            ..Default::default()
        }]),
        ..Default::default()
    }
}

fn job_list(jobs: &[Job]) -> Result<FakeReply, serde_json::Error> {
    FakeReply::list("batch/v1", "JobList", jobs)
}

/// Runs the watcher over the scripted replies and returns the first status it
/// publishes, stopping the watcher afterwards.
async fn first_status(replies: Vec<FakeReply>) -> Result<SandboxStatus, Box<dyn std::error::Error>> {
    let api = FakeApiServer::new(replies);
    let (tx, mut rx) = watch::channel(SandboxStatus::Pending);
    let handle = api.ops().spawn_job_watcher("work", "rs-job", tx);

    let changed = tokio::time::timeout(Duration::from_secs(10), rx.changed()).await;
    handle.abort();
    changed??;
    let status = rx.borrow().clone();
    Ok(status)
}

#[tokio::test]
async fn a_complete_job_publishes_completed() -> TestResult {
    let status = first_status(vec![
        job_list(&[with_status("rs-job", condition("Complete", None))])?,
        FakeReply::hanging(),
    ])
    .await?;

    assert_eq!(status, SandboxStatus::Completed);
    Ok(())
}

#[tokio::test]
async fn a_failed_job_publishes_its_reason() -> TestResult {
    let status = first_status(vec![
        job_list(&[with_status("rs-job", condition("Failed", Some("out of time")))])?,
        FakeReply::hanging(),
    ])
    .await?;

    assert_eq!(
        status,
        SandboxStatus::Failed {
            error: "out of time".to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_job_with_active_pods_publishes_running() -> TestResult {
    let status = first_status(vec![
        job_list(&[with_status(
            "rs-job",
            JobStatus {
                active: Some(1),
                ..Default::default()
            },
        )])?,
        FakeReply::hanging(),
    ])
    .await?;

    assert_eq!(status, SandboxStatus::Running);
    Ok(())
}

#[tokio::test]
async fn a_new_job_publishes_pending_and_watch_errors_are_retried() -> TestResult {
    let status = first_status(vec![
        FakeReply::failure(500, "InternalError"),
        job_list(&[job("rs-job")])?,
        FakeReply::hanging(),
    ])
    .await?;

    assert_eq!(status, SandboxStatus::Pending);
    Ok(())
}

#[tokio::test]
async fn a_deleted_job_publishes_cancelled_and_ends_the_watcher() -> TestResult {
    let mut gone = job("rs-job");
    gone.metadata.resource_version = Some("2".to_string());
    let api = FakeApiServer::new(vec![
        job_list(&[])?,
        FakeReply::watch_events(&[&format!(
            r#"{{"type":"DELETED","object":{}}}"#,
            serde_json::to_string(&gone)?
        )]),
    ]);
    let (tx, rx) = watch::channel(SandboxStatus::Pending);

    let handle = api.ops().spawn_job_watcher("work", "rs-job", tx);
    tokio::time::timeout(Duration::from_secs(10), handle).await??;

    assert_eq!(*rx.borrow(), SandboxStatus::Cancelled);
    let seen = api.seen();
    assert!(
        seen[0].uri.contains("fieldSelector=metadata.name%3Drs-job"),
        "{}",
        seen[0].uri
    );
    assert!(seen[1].uri.contains("watch=true"), "{}", seen[1].uri);
    Ok(())
}

#[tokio::test]
async fn the_watcher_ends_once_nobody_listens() -> TestResult {
    let api = FakeApiServer::new(vec![
        job_list(&[with_status("rs-job", condition("Complete", None))])?,
        FakeReply::hanging(),
    ]);
    let (tx, rx) = watch::channel(SandboxStatus::Pending);
    drop(rx);

    let handle = api.ops().spawn_job_watcher("work", "rs-job", tx);

    tokio::time::timeout(Duration::from_secs(10), handle).await??;
    Ok(())
}
