mod support;

use std::time::Duration;

use serde_json::json;
use support::*;
use trogon_sandbox_contract::{LeaseError, NativeHandleResponse, ProviderKind, SandboxId, WorkerId};
use trogon_sandbox_daytona_worker::{DaytonaApiError, DaytonaProviderError, SandboxState};
use trogon_sandbox_protocol::{Provider, ProviderError};

fn provider_error(error: &ProviderError) -> Option<&DaytonaProviderError> {
    match error {
        ProviderError::Backend(inner) => inner.downcast_ref::<DaytonaProviderError>(),
        _ => None,
    }
}

fn create_body(fake: &Fake) -> Result<serde_json::Value, Boxed> {
    Ok(fake.calls("POST", "/sandbox").first().ok_or("no create call")?.json())
}

#[tokio::test]
async fn an_empty_workspace_lease_creates_waits_and_makes_the_workspace() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    assert_eq!(provider.kind(), ProviderKind::Daytona);
    let lease = within(provider.lease(&request()?, &lease_id("l1"))).await??;
    assert_eq!(lease.sandbox_id, SandboxId::new(SANDBOX)?);
    assert!(lease.evidence_subject.contains("l1"));
    assert_eq!(provider.active_lease_count().await, 1);
    let body = create_body(&fake)?;
    assert!(
        body["snapshot"]
            .as_str()
            .is_some_and(|name| name.contains("sandbox-agent"))
    );
    assert_eq!(body["autoStopInterval"], 0);
    let folder = fake.calls("POST", &format!("/box/{SANDBOX}/files/folder"));
    assert_eq!(
        folder.first().ok_or("no folder call")?.query,
        "path=%2Fworkspace&mode=755"
    );
    let held = provider.sandbox_for_lease(&lease_id("l1")).await.ok_or("not held")?;
    assert_eq!(held.id, SANDBOX);
    assert!(provider.sandbox_for_lease(&lease_id("other")).await.is_none());
    Ok(())
}

#[tokio::test]
async fn the_request_shapes_the_create_body() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let request = request_with(|wire| {
        wire["spec"]["image"]["reference"] = json!({"registry": "", "repository": "agent", "tag": "dev"});
        wire["spec"]["env"] = json!({"A": "1"});
        wire["spec"]["idle_timeout_seconds"] = json!(90);
        wire["tags"] = json!({"team": "x"});
        wire["region"] = json!("eu");
        wire["native"] = json!({"daytona": {"public": true, "auto_archive_interval": 30, "auto_stop_interval": 7, "class": "large", "gpu": 2}});
    })?;
    fake.provider()?.lease(&request, &lease_id("l1")).await?;
    let body = create_body(&fake)?;
    assert!(body["snapshot"].as_str().is_some_and(|name| name.contains("agent")));
    assert_eq!(body["env"], json!({"A": "1"}));
    assert_eq!(body["labels"], json!({"trogon.tag.team": "x", "trogon.lease_id": "l1"}));
    assert_eq!(body["class"], "large");
    assert_eq!(body["gpu"], 2);
    assert_eq!(body["target"], "eu");
    assert_eq!(body["public"], true);
    assert_eq!(body["autoStopInterval"], 7);
    assert_eq!(body["autoArchiveInterval"], 30);
    assert!(body.get("networkBlockAll").is_none());
    let idle = request_with(|wire| wire["spec"]["idle_timeout_seconds"] = json!(90))?;
    let second = Fake::with_sandbox().await?;
    second.provider()?.lease(&idle, &lease_id("l2")).await?;
    assert_eq!(create_body(&second)?["autoStopInterval"], 2);
    Ok(())
}

#[tokio::test]
async fn a_native_snapshot_names_the_base() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    let restore = request_with(|wire| {
        wire["spec"]["image"] =
            json!({"kind": "native_snapshot", "reference": {"provider": "daytona", "id": "snap_1"}});
    })?;
    provider.lease(&restore, &lease_id("l1")).await?;
    assert_eq!(create_body(&fake)?["snapshot"], "snap_1");
    let foreign = request_with(|wire| {
        wire["spec"]["image"] = json!({"kind": "native_snapshot", "reference": {"provider": "vercel", "id": "x"}});
    })?;
    let refused = provider
        .lease(&foreign, &lease_id("l2"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(DaytonaProviderError::Lease(LeaseError::InvalidSpec { .. }))
    ));
    assert_eq!(fake.count("POST", "/sandbox"), 1);
    Ok(())
}

#[tokio::test]
async fn a_clone_workspace_clones_at_the_commit_with_credentials() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", &format!("/box/{SANDBOX}/git/clone"), 200, json!({}));
    let token = key();
    let request = request_with(|wire| {
        wire["spec"]["workspace"] = json!({"clone": {
            "url": "https://git.example.com/org/repo.git",
            "sha": "0123456789abcdef",
            "base_sha": null,
            "auth": {"bearer_token": {"token_ref": {"literal": {"value": token}}}}
        }});
    })?;
    fake.provider()?.lease(&request, &lease_id("l1")).await?;
    let calls = fake.calls("POST", &format!("/box/{SANDBOX}/git/clone"));
    assert_eq!(
        calls.first().ok_or("no clone call")?.json(),
        json!({
            "url": "https://git.example.com/org/repo.git",
            "path": "/workspace",
            "commit_id": "0123456789abcdef",
            "username": "oauth2",
            "password": token,
        })
    );
    assert_eq!(fake.count("POST", &format!("/box/{SANDBOX}/files/folder")), 0);
    Ok(())
}

#[tokio::test]
async fn refusals_happen_before_any_create_call() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;

    let unknown = request_with(|wire| wire["native"] = json!({"daytona": {"typo": 1}}))?;
    let refused = provider.lease(&unknown, &lease_id("a")).await.err().ok_or("accepted")?;
    assert!(matches!(refused, ProviderError::NativeOptions(_)));

    let privileged = request_with(|wire| wire["spec"]["privileged"] = json!(true))?;
    let refused = provider
        .lease(&privileged, &lease_id("p"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(refused, ProviderError::PrivilegedUnsupported(_)));

    let mounts = request_with(|wire| {
        wire["spec"]["mounts"] =
            json!([{"source": {"backend": "s3", "bucket": "bucket-a", "prefix": "p"}, "mount_path": "/mnt"}]);
    })?;
    let refused = provider.lease(&mounts, &lease_id("m")).await.err().ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(DaytonaProviderError::MountsUnsupported)
    ));

    let denied = request_with(|wire| {
        wire["spec"]["network_policy"] = json!({
            "user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": [], "denied_domains": ["bad.example.com"]}
        });
    })?;
    let refused = provider.lease(&denied, &lease_id("n")).await.err().ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(DaytonaProviderError::Lease(LeaseError::InvalidNetworkPolicy { .. }))
    ));

    let size = request_with(|wire| wire["spec"]["vcpu_millis"] = json!(2000))?;
    let disk = request_with(|wire| wire["spec"]["disk_mib"] = json!(1024))?;
    let insecure = request_with(|wire| {
        wire["spec"]["workspace"] = json!({"clone": {
            "url": "http://git.example.com/r.git", "sha": "0123456789abcdef", "base_sha": null, "auth": {"none": {}}
        }});
    })?;
    let snapshot_tag = request_with(|wire| {
        wire["spec"]["image"] = json!({
            "kind": "snapshot_tag",
            "tag": {"tenant": "t", "name": "prod"},
            "artifact_type": "sandbox_rootfs_v1"
        });
    })?;
    for (name, invalid) in [("s", size), ("d", disk), ("c", insecure), ("t", snapshot_tag)] {
        let refused = provider
            .lease(&invalid, &lease_id(name))
            .await
            .err()
            .ok_or("accepted")?;
        assert!(matches!(provider_error(&refused), Some(DaytonaProviderError::Lease(_))));
    }
    assert_eq!(fake.total(), 0);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_vendor_failure_surfaces_and_registers_nothing() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", "/sandbox", 429, json!({"message": "slow down"}));
    let provider = fake.provider()?;
    let refused = provider
        .lease(&request()?, &lease_id("l1"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(DaytonaProviderError::Api(DaytonaApiError::RateLimited { .. }))
    ));
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_sandbox_that_starts_late_is_started_first() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let get = format!("/sandbox/{SANDBOX}");
    fake.script(
        "GET",
        &get,
        vec![
            Reply::json(200, record("stopped")),
            Reply::json(200, record("starting")),
            Reply::json(200, record("started")),
        ],
    );
    fake.json(
        "POST",
        &format!("{get}/start"),
        409,
        json!({"message": "already moving"}),
    );
    fake.provider()?.lease(&request()?, &lease_id("l1")).await?;
    assert_eq!(fake.count("POST", &format!("{get}/start")), 1);
    Ok(())
}

#[tokio::test]
async fn a_sandbox_that_fails_to_start_is_deleted_and_not_held() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json(
        "GET",
        &format!("/sandbox/{SANDBOX}"),
        200,
        json!({"id": SANDBOX, "state": "error", "errorReason": "no capacity"}),
    );
    let provider = fake.provider()?;
    let refused = provider
        .lease(&request()?, &lease_id("l1"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(DaytonaProviderError::SandboxFailed {
            state: SandboxState::Error,
            ..
        })
    ));
    assert_eq!(fake.count("DELETE", &format!("/sandbox/{SANDBOX}")), 1);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_sandbox_that_never_starts_times_out_and_is_deleted() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("GET", &format!("/sandbox/{SANDBOX}"), 200, record("starting"));
    let provider = fake.provider_waiting(Duration::from_millis(150))?;
    let refused = provider
        .lease(&request()?, &lease_id("l1"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(DaytonaProviderError::SettleTimedOut {
            state: SandboxState::Starting,
            ..
        })
    ));
    assert_eq!(fake.count("DELETE", &format!("/sandbox/{SANDBOX}")), 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_workspace_setup_deletes_the_sandbox() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", &format!("/box/{SANDBOX}/files/folder"), 500, json!({}));
    let provider = fake.provider()?;
    assert!(provider.lease(&request()?, &lease_id("l1")).await.is_err());
    assert_eq!(fake.count("DELETE", &format!("/sandbox/{SANDBOX}")), 1);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn an_id_the_protocol_refuses_is_a_backend_failure() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", "/sandbox", 200, json!({"id": "", "state": "started"}));
    let provider = fake.provider()?;
    let refused = provider
        .lease(&request()?, &lease_id("l1"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(DaytonaProviderError::SandboxId(_))
    ));
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_second_lease_for_a_held_id_is_refused() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    let refused = provider
        .lease(&request()?, &lease_id("l1"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(DaytonaProviderError::AlreadyHeld)
    ));
    assert_eq!(fake.count("POST", "/sandbox"), 1);
    Ok(())
}

#[tokio::test]
async fn concurrent_leases_for_one_id_share_a_gate_and_only_one_creates() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    let (one, two, id) = (request()?, request()?, lease_id("same"));
    let (first, second) = tokio::join!(provider.lease(&one, &id), provider.lease(&two, &id));
    assert!(first.is_ok() != second.is_ok());
    assert_eq!(fake.count("POST", "/sandbox"), 1);
    assert_eq!(provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn the_receipt_names_the_bound_worker_and_binding_is_stable() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    let unbound = provider.lease(&request()?, &lease_id("l1")).await?;
    assert!(unbound.worker_id.is_none());
    let worker = WorkerId::new("worker-1")?;
    provider.bind_worker(&worker)?;
    provider.bind_worker(&worker)?;
    let refused = provider
        .bind_worker(&WorkerId::new("worker-2")?)
        .err()
        .ok_or("rebound")?;
    assert!(matches!(refused, DaytonaProviderError::BoundToAnotherWorker));
    let bound = provider.lease(&request()?, &lease_id("l2")).await?;
    assert_eq!(bound.worker_id, Some(worker));
    Ok(())
}

#[tokio::test]
async fn the_native_handle_names_the_sandbox_and_its_region() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    assert!(matches!(
        provider.native_handle_for_lease(&lease_id("l1")).await,
        NativeHandleResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    provider.lease(&request()?, &lease_id("l1")).await?;
    assert!(matches!(
        provider.native_handle_for_lease(&lease_id("l1")).await,
        NativeHandleResponse::Available(ref held)
            if held.vendor_id == SANDBOX && held.region.as_deref() == Some("us") && held.provider == ProviderKind::Daytona
    ));
    Ok(())
}

#[tokio::test]
async fn the_request_region_stands_in_when_daytona_names_none() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", "/sandbox", 200, json!({"id": SANDBOX, "state": "started"}));
    let provider = fake.provider()?;
    let regional = request_with(|wire| wire["region"] = json!("sfo"))?;
    provider.lease(&regional, &lease_id("l1")).await?;
    assert!(matches!(
        provider.native_handle_for_lease(&lease_id("l1")).await,
        NativeHandleResponse::Available(ref held) if held.region.as_deref() == Some("sfo")
    ));
    Ok(())
}

#[tokio::test]
async fn the_native_handle_points_at_the_sandbox_page() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request_with(|_| {})?, &lease_id("l1")).await?;
    let NativeHandleResponse::Available(handle) = provider.native_handle_for_lease(&lease_id("l1")).await else {
        return Err("no handle".into());
    };
    let url = handle.url.ok_or("no url")?;
    assert!(url.ends_with(&format!("/sandbox/{SANDBOX}")), "{url}");
    Ok(())
}
