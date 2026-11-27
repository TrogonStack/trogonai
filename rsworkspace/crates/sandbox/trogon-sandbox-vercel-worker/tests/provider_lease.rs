mod provider_support;
mod support;

use provider_support::*;
use serde_json::json;
use trogon_sandbox_contract::{LeaseError, ProviderKind, SandboxId, WorkerId};
use trogon_sandbox_protocol::{Provider, ProviderError};
use trogon_sandbox_vercel_worker::{VercelApiError, VercelPlan, VercelProviderError};

fn provider_error(error: &ProviderError) -> Option<&VercelProviderError> {
    match error {
        ProviderError::Backend(inner) => inner.downcast_ref::<VercelProviderError>(),
        _ => None,
    }
}

#[tokio::test]
async fn a_lease_creates_a_sandbox_from_the_named_runtime() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let provider = fake.provider()?;
    assert_eq!(provider.kind(), ProviderKind::Vercel);
    let lease = within(provider.lease(&request()?, &lease_id("l1"))).await??;
    assert_eq!(lease.sandbox_id, SandboxId::new("sbx-a")?);
    assert_eq!(provider.active_lease_count().await, 1);

    let calls = fake.calls("POST", "/v2/sandboxes");
    let call = calls.first().ok_or("no create call")?;
    assert!(call.path.contains("teamId=team_1"));
    assert_eq!(call.body["projectId"], "prj_1");
    assert_eq!(call.body["runtime"], "python3.13");
    assert_eq!(call.body["timeout"], 600_000);
    assert_eq!(call.body["persistent"], true);

    let sandbox = provider
        .sandbox_for_lease(&lease_id("l1"))
        .await
        .ok_or("lease not held")?;
    assert_eq!(sandbox.session_id, "sess_1");
    assert_eq!(
        sandbox.routes.get(&3000).map(String::as_str),
        Some("https://one.example.test")
    );
    assert!(provider.sandbox_for_lease(&lease_id("other")).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_managed_image_path_is_sent_as_image_not_runtime() -> TestResult {
    let fake = Fake::start().await?;
    fake.on("POST", "/v3/sandboxes", 200, record("s", "https://x.test"));
    let request = request_with(|wire| {
        wire["spec"]["image"]["reference"]["repository"] = json!("vercel/sandbox/universal");
    })?;
    fake.provider()?.lease(&request, &lease_id("l1")).await?;
    let calls = fake.calls("POST", "/v3/sandboxes");
    let body = &calls.first().ok_or("no create call")?.body;
    assert_eq!(body["image"], "vercel/sandbox/universal");
    assert!(body.get("runtime").is_none());
    Ok(())
}

#[tokio::test]
async fn tags_reach_the_create_body_and_long_values_are_refused() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let provider = fake.provider()?;
    let tagged = request_with(|wire| wire["tags"] = json!({"team": "x"}))?;
    provider.lease(&tagged, &lease_id("l1")).await?;
    let calls = fake.calls("POST", "/v2/sandboxes");
    assert_eq!(
        calls.first().ok_or("no create call")?.body["tags"],
        json!({"team": "x"})
    );

    let long = request_with(|wire| wire["tags"] = json!({"team": "x".repeat(257)}))?;
    let refused = provider.lease(&long, &lease_id("l2")).await.err().ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::Lease(LeaseError::InvalidSpec { .. }))
    ));
    assert_eq!(fake.count("POST", "/v2/sandboxes"), 1);
    Ok(())
}

#[tokio::test]
async fn a_native_snapshot_restores_through_the_snapshot_source() -> TestResult {
    let fake = Fake::start().await?;
    fake.on("POST", "/v3/sandboxes", 200, record("s", "https://x.test"));
    let provider = fake.provider()?;
    let restore = request_with(|wire| {
        wire["spec"]["image"] = json!({"kind": "native_snapshot", "reference": {"provider": "vercel", "id": "snap_1"}});
    })?;
    provider.lease(&restore, &lease_id("l1")).await?;
    let calls = fake.calls("POST", "/v3/sandboxes");
    assert_eq!(
        calls.first().ok_or("no create call")?.body["source"],
        json!({"type": "snapshot", "snapshotId": "snap_1"})
    );

    let foreign = request_with(|wire| {
        wire["spec"]["image"] = json!({"kind": "native_snapshot", "reference": {"provider": "daytona", "id": "x"}});
    })?;
    let refused = provider
        .lease(&foreign, &lease_id("l2"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::Lease(LeaseError::InvalidSpec { .. }))
    ));
    assert_eq!(fake.count("POST", "/v3/sandboxes"), 1);
    Ok(())
}

#[tokio::test]
async fn a_lease_past_the_plan_cap_is_refused_before_any_create_call() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let hobby = fake.provider_on(VercelPlan::Hobby)?;
    let long = request_with(|wire| wire["ttl_seconds"] = json!(2701))?;
    let refused = hobby.lease(&long, &lease_id("l1")).await.err().ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::Lease(LeaseError::WouldExceedMaxLifetime {
            max: 2700
        }))
    ));

    let unbounded = request_with(|wire| {
        wire["spec"]["no_ttl"] = json!(true);
        wire["spec"]["idle_timeout_seconds"] = json!(60);
    })?;
    let refused = hobby.lease(&unbounded, &lease_id("l1")).await.err().ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::Lease(LeaseError::WouldExceedMaxLifetime {
            max: 2700
        }))
    ));

    let wide = request_with(|wire| wire["spec"]["vcpu_millis"] = json!(5000))?;
    let refused = hobby.lease(&wide, &lease_id("l1")).await.err().ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::Lease(LeaseError::InvalidSpec { .. }))
    ));
    assert_eq!(fake.total(), 0);
    Ok(())
}

#[tokio::test]
async fn size_ports_env_region_and_network_reach_the_create_body() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let request = request_with(|wire| {
        wire["spec"]["vcpu_millis"] = json!(3200);
        wire["spec"]["memory_mib"] = json!(4096);
        wire["spec"]["exposed_ports"] = json!([3000, 8080]);
        wire["spec"]["env"] = json!({"A": "1"});
        wire["spec"]["network_policy"] = json!("deny_all");
        wire["region"] = json!("iad1");
        wire["native"] = json!({"vercel": {"persistent": false}});
    })?;
    fake.provider()?.lease(&request, &lease_id("l1")).await?;
    let calls = fake.calls("POST", "/v2/sandboxes");
    let body = &calls.first().ok_or("no create call")?.body;
    assert_eq!(body["resources"], json!({"vcpus": 4}));
    assert_eq!(body["ports"], json!([3000, 8080]));
    assert_eq!(body["env"], json!({"A": "1"}));
    assert_eq!(body["region"], "iad1");
    assert_eq!(body["networkPolicy"], json!({"mode": "deny-all"}));
    assert_eq!(body["persistent"], false);
    Ok(())
}

#[tokio::test]
async fn refusals_happen_before_any_create_call() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let provider = fake.provider()?;

    let unknown = request_with(|wire| wire["native"] = json!({"vercel": {"typo": 1}}))?;
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
        Some(VercelProviderError::MountsUnsupported)
    ));

    let denied = request_with(|wire| {
        wire["spec"]["network_policy"] = json!({
            "user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": [], "denied_domains": ["bad.example.com"]}
        });
    })?;
    let refused = provider.lease(&denied, &lease_id("n")).await.err().ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::Lease(LeaseError::InvalidNetworkPolicy { .. }))
    ));

    let big = request_with(|wire| wire["spec"]["vcpu_millis"] = json!(9000))?;
    let disk = request_with(|wire| wire["spec"]["disk_mib"] = json!(1024))?;
    let snapshot_tag = request_with(|wire| {
        wire["spec"]["image"] = json!({
            "kind": "snapshot_tag",
            "tag": {"tenant": "t", "name": "prod"},
            "artifact_type": "sandbox_rootfs_v1"
        });
    })?;
    let long_image = request_with(|wire| {
        wire["spec"]["image"]["reference"]["repository"] = json!("a".repeat(300));
    })?;
    for (name, invalid) in [("big", big), ("d", disk), ("c", snapshot_tag), ("img", long_image)] {
        let refused = provider
            .lease(&invalid, &lease_id(name))
            .await
            .err()
            .ok_or("accepted")?;
        assert!(matches!(provider_error(&refused), Some(VercelProviderError::Lease(_))));
    }
    assert_eq!(fake.total(), 0);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_vendor_failure_surfaces_and_registers_nothing() -> TestResult {
    let fake = Fake::start().await?;
    fake.on("POST", "/v2/sandboxes", 429, json!({"error": {"message": "slow down"}}));
    let provider = fake.provider()?;
    let refused = provider
        .lease(&request()?, &lease_id("l1"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::Api(VercelApiError::RateLimited {
            retry_after_secs: None,
            ..
        }))
    ));
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_second_lease_for_a_held_id_is_refused() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    let refused = provider
        .lease(&request()?, &lease_id("l1"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::AlreadyHeld)
    ));
    assert_eq!(fake.count("POST", "/v2/sandboxes"), 1);
    Ok(())
}

#[tokio::test]
async fn concurrent_leases_for_one_id_share_a_gate_and_only_one_creates() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let provider = fake.provider()?;
    let (one, two, id) = (request()?, request()?, lease_id("same"));
    let (first, second) = tokio::join!(provider.lease(&one, &id), provider.lease(&two, &id));
    assert!(first.is_ok() != second.is_ok());
    assert_eq!(fake.count("POST", "/v2/sandboxes"), 1);
    assert_eq!(provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn a_sandbox_name_the_protocol_refuses_is_a_backend_failure() -> TestResult {
    let fake = Fake::start().await?;
    fake.on(
        "POST",
        "/v2/sandboxes",
        200,
        json!({"sandbox": {"name": ""}, "session": {"id": "sess_1"}}),
    );
    let provider = fake.provider()?;
    let refused = provider
        .lease(&request()?, &lease_id("l1"))
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        provider_error(&refused),
        Some(VercelProviderError::SandboxId(_))
    ));
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn the_receipt_names_the_bound_worker_and_binding_is_stable() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let provider = fake.provider()?;
    let unbound = provider.lease(&request()?, &lease_id("l1")).await?;
    assert!(unbound.worker_id.is_none());
    assert!(unbound.evidence_subject.as_str().contains("l1"));

    let worker = WorkerId::new("worker-1")?;
    provider.bind_worker(&worker)?;
    provider.bind_worker(&worker)?;
    let refused = provider
        .bind_worker(&WorkerId::new("worker-2")?)
        .err()
        .ok_or("rebound")?;
    assert!(matches!(refused, VercelProviderError::BoundToAnotherWorker));
    let bound = provider.lease(&request()?, &lease_id("l2")).await?;
    assert_eq!(bound.worker_id, Some(worker));
    Ok(())
}

#[tokio::test]
async fn the_request_region_stands_in_when_the_session_names_none() -> TestResult {
    let fake = Fake::start().await?;
    fake.on(
        "POST",
        "/v2/sandboxes",
        200,
        json!({"sandbox": {"name": "sbx-a"}, "session": {"id": "sess_1"}}),
    );
    let provider = fake.provider()?;
    let regional = request_with(|wire| wire["region"] = json!("sfo1"))?;
    provider.lease(&regional, &lease_id("l1")).await?;
    let handle = provider.native_handle_for_lease(&lease_id("l1")).await;
    assert!(matches!(
        handle,
        trogon_sandbox_contract::NativeHandleResponse::Available(ref held) if held.region.as_deref() == Some("sfo1")
    ));
    Ok(())
}
