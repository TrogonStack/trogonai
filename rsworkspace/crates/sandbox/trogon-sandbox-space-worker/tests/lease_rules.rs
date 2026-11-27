mod support;

use serde_json::{Value, json};
use support::{Rig, TestResult, dispatch_json};
use trogon_sandbox_contract::{
    ProviderDispatchPayload, RunCommandRequest, RunCommandResponse, SandboxFileWrite, WriteFilesRequest,
};
use trogon_sandbox_space_worker::{LeaseViolationError, SpaceProviderError};

fn with(change: impl FnOnce(&mut Value)) -> Result<ProviderDispatchPayload, Box<dyn std::error::Error>> {
    let mut body = dispatch_json("lease-1", None);
    change(&mut body);
    Ok(serde_json::from_value(body)?)
}

async fn refused(payload: ProviderDispatchPayload) -> Result<LeaseViolationError, Box<dyn std::error::Error>> {
    let rig = Rig::start().await?;
    match rig.provider.lease_dispatch(&payload).await {
        Err(SpaceProviderError::Violation(violation)) => Ok(violation),
        other => Err(format!("expected a refusal, got {other:?}").into()),
    }
}

#[tokio::test]
async fn refuses_an_image_the_template_does_not_serve() -> TestResult {
    let payload = with(|body| body["request"]["spec"]["image"]["reference"]["repository"] = json!("other"))?;
    assert!(matches!(refused(payload).await?, LeaseViolationError::Template));
    Ok(())
}

#[tokio::test]
async fn refuses_privileged_leases() -> TestResult {
    let payload = with(|body| body["request"]["spec"]["privileged"] = json!(true))?;
    assert!(matches!(refused(payload).await?, LeaseViolationError::Privileged));
    Ok(())
}

#[tokio::test]
async fn refuses_a_workspace_that_is_not_empty() -> TestResult {
    let payload =
        with(|body| body["request"]["spec"]["workspace"] = json!({"host_mount": {"path": "/srv", "readonly": true}}))?;
    assert!(matches!(refused(payload).await?, LeaseViolationError::Workspace));
    Ok(())
}

#[tokio::test]
async fn refuses_exposed_ports() -> TestResult {
    let payload = with(|body| body["request"]["spec"]["exposed_ports"] = json!([8080]))?;
    assert!(matches!(refused(payload).await?, LeaseViolationError::Ports));
    Ok(())
}

#[tokio::test]
async fn refuses_an_egress_policy() -> TestResult {
    let payload = with(|body| body["request"]["spec"]["network_policy"] = json!("deny_all"))?;
    assert!(matches!(refused(payload).await?, LeaseViolationError::Egress));
    Ok(())
}

#[tokio::test]
async fn refuses_policies_it_cannot_enforce() -> TestResult {
    let payload = with(|body| body["request"]["spec"]["user"] = json!("root"))?;
    assert!(matches!(refused(payload).await?, LeaseViolationError::Policies));
    Ok(())
}

#[tokio::test]
async fn refuses_a_relative_working_directory() -> TestResult {
    let payload = with(|body| body["request"]["spec"]["working_dir"] = json!("work"))?;
    assert!(matches!(refused(payload).await?, LeaseViolationError::WorkingDirectory));
    Ok(())
}

#[tokio::test]
async fn sizes_the_sandbox_from_the_spec() -> TestResult {
    let rig = Rig::start().await?;
    let payload = with(|body| {
        body["request"]["spec"]["vcpu_millis"] = json!(1500);
        body["request"]["spec"]["disk_mib"] = json!(2048);
        body["request"]["spec"]["working_dir"] = json!("/work");
    })?;
    rig.provider.lease_dispatch(&payload).await?;
    let state = rig.space.state();
    let body = state.create_bodies.first().ok_or("no create")?;
    assert_eq!(body["resources"]["vcpu"], 2);
    assert_eq!(body["resources"]["disk_mib"], 2048);
    assert_eq!(body["resources"]["memory_mib"], 512);
    Ok(())
}

#[tokio::test]
async fn resolves_paths_and_commands_against_the_lease_directory() -> TestResult {
    let rig = Rig::start().await?;
    let payload = with(|body| body["request"]["spec"]["working_dir"] = json!("/work/"))?;
    rig.provider.lease_dispatch(&payload).await?;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id": "lease-1", "command": ["pwd"], "cwd": "sub", "timeout_seconds": 5,
    }))?;
    assert!(matches!(
        rig.provider.run_command(&request).await,
        RunCommandResponse::Completed(_)
    ));
    assert_eq!(
        rig.space.state().commands.last().ok_or("no command")?["cwd"],
        "/work/sub"
    );
    let write = WriteFilesRequest {
        lease_id: payload.lease_id.clone(),
        files: vec![SandboxFileWrite {
            path: "notes.txt".into(),
            content: vec![1],
            mode: None,
        }],
        as_user: None,
    };
    rig.provider.write_files(&write).await?;
    assert_eq!(rig.space.state().fs.get("/work/notes.txt"), Some(&vec![1]));
    Ok(())
}

#[tokio::test]
async fn refuses_native_volumes() -> TestResult {
    let payload = with(|body| {
        body["resolved_drives"] = json!([{
            "id": {"tenant": "acme", "name": "drive-1"}, "tenant": "acme", "size_gib": 1,
            "created_at": {"secs_since_epoch": 1, "nanos_since_epoch": 0},
            "attached_lease": null, "bucket": "b",
        }]);
    })?;
    assert!(matches!(refused(payload).await?, LeaseViolationError::Volumes));
    Ok(())
}
