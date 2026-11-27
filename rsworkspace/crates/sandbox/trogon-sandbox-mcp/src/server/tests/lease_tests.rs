use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use trogon_sandbox_client::BusMessage;
use trogon_sandbox_contract::{
    AccountLimits, ExtendLeaseTimeoutResponse, GetAccountLimitsResponse, GetLeaseUsageResponse, LeaseCreation,
    LeaseGranted, LeaseId, LeaseRequest, LeaseRequestKey, LeaseResponse, LeaseStatus, LeaseUsage,
    ListCapabilitiesResponse, ListLeasesResponse, OciImageRef, ProviderKind, ReapLeasesResponse, ReapMode,
    SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects;

use super::support::{call, call_with};
use crate::policy::Policy;
use crate::testing::{McpSession, Reply, ScriptedBus, TestResult, test_server};

fn granted() -> LeaseGranted {
    LeaseGranted {
        lease_id: LeaseId::new("lease-1"),
        provider: ProviderKind::Mock,
        sandbox_id: "sandbox-1".into(),
        evidence_subject: "sandbox.evidence.lease-1.>".into(),
        deadline: SystemTime::now() + Duration::from_secs(300),
        created: LeaseCreation::Created,
        capabilities: Vec::new(),
        entity_id: None,
        operation_id: None,
    }
}

fn lease_request() -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("alpine"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: Default::default(),
        cpu_seconds: NonZeroU32::MIN,
        memory_mib: NonZeroU32::MIN,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?;
    Ok(LeaseRequest {
        tenant: "someone-else".parse()?,
        provider_pref: vec![ProviderKind::Mock],
        spec,
        ttl_seconds: 60,
        idempotency_key: LeaseRequestKey::new("key-1")?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: Default::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: Default::default(),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
        operation_id: None,
    })
}

fn usage() -> Result<LeaseUsage, Box<dyn std::error::Error>> {
    Ok(LeaseUsage {
        lease_id: LeaseId::new("l1"),
        tenant: "acme".parse()?,
        status: LeaseStatus::Running,
        created_at: SystemTime::UNIX_EPOCH,
        ended_at: None,
        last_activity_at: SystemTime::UNIX_EPOCH,
        commands_run: 3,
        command_output_bytes: 0,
        file_bytes_in: 0,
        file_bytes_out: 0,
        running_seconds: 0,
        paused_seconds: 0,
        suspended_seconds: 0,
        suspend_artifact_bytes: None,
        gauges: None,
    })
}

#[tokio::test]
async fn a_lease_is_requested_for_the_server_tenant() -> TestResult {
    let args = serde_json::to_value(lease_request()?)?;
    let outcome = call("sandbox_lease", args, &LeaseResponse::Granted(granted())).await?;
    assert!(!outcome.is_error());
    let sent = outcome.sent.first().ok_or("nothing was sent")?;
    assert_eq!(sent.subject, subjects::LEASE_REQUEST);
    assert_eq!(sent.json()?["tenant"], json!("acme"));
    assert!(outcome.text()?.contains("lease-1"));
    Ok(())
}

#[tokio::test]
async fn a_malformed_lease_request_never_reaches_the_bus() -> TestResult {
    let outcome = call(
        "sandbox_lease",
        json!({"ttl_seconds": "soon"}),
        &LeaseResponse::Granted(granted()),
    )
    .await?;
    assert!(outcome.is_error());
    assert!(outcome.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_cancel_is_acknowledged_even_when_nobody_answers() -> TestResult {
    let outcome = call_with(
        Policy::default(),
        "sandbox_cancel",
        json!({"lease_id": "l1"}),
        vec![Reply::Timeout],
    )
    .await?;
    assert!(!outcome.is_error());
    let sent = outcome.sent.first().ok_or("nothing was sent")?;
    assert_eq!(sent.subject, subjects::lease_cancel(&LeaseId::new("l1")));
    Ok(())
}

async fn wait_released(items: Vec<BusMessage>, seconds: u32) -> Result<(bool, String), Box<dyn std::error::Error>> {
    let bus = ScriptedBus::silent().with_subscribe_items(items);
    let session = McpSession::start(test_server(bus, Policy::default())?).await?;
    let result = session
        .call(
            "sandbox_wait_released",
            json!({"lease_id": "l1", "timeout_seconds": seconds}),
        )
        .await?;
    session.finish().await?;
    Ok((
        result.is_error == Some(true),
        crate::testing::call_result_text(&result)?,
    ))
}

#[tokio::test]
async fn wait_released_reports_why_the_lease_ended() -> TestResult {
    let released = json!({"lease_id": "l1", "tenant": "acme", "reason": "expired"});
    let message = BusMessage {
        subject: subjects::lease_released(&LeaseId::new("l1")),
        payload: serde_json::to_vec(&released)?.into(),
    };
    let (is_error, text) = wait_released(vec![message], 5).await?;
    assert!(!is_error);
    assert!(text.contains("expired"), "{text}");
    Ok(())
}

#[tokio::test]
async fn wait_released_fails_when_nothing_is_announced() -> TestResult {
    let (is_error, _) = wait_released(Vec::new(), 0).await?;
    assert!(is_error);
    Ok(())
}

#[tokio::test]
async fn listing_leases_sends_the_filters_for_the_tenant() -> TestResult {
    let reply = ListLeasesResponse {
        leases: Vec::new(),
        next_cursor: None,
        denied: None,
    };
    let args = json!({"status_filter": "running", "limit": 5, "tags": {"team": "a"}});
    let outcome = call("sandbox_list_leases", args, &reply).await?;
    assert!(!outcome.is_error());
    let body = outcome.sent.first().ok_or("nothing was sent")?.json()?;
    assert_eq!(
        (body["tenant"].clone(), body["limit"].clone()),
        (json!("acme"), json!(5))
    );
    assert_eq!(body["tags"]["team"], json!("a"));
    Ok(())
}

#[tokio::test]
async fn listing_leases_rejects_an_unknown_status() -> TestResult {
    let reply = ListLeasesResponse {
        leases: Vec::new(),
        next_cursor: None,
        denied: None,
    };
    let outcome = call("sandbox_list_leases", json!({"status_filter": "bogus"}), &reply).await?;
    assert!(outcome.is_error());
    assert!(outcome.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn reaping_needs_a_filter() -> TestResult {
    let reply = ReapLeasesResponse::Reaped {
        dry_run: ReapMode::DryRun,
        matched: Vec::new(),
        released: Vec::new(),
        failed: Vec::new(),
    };
    let none = call("sandbox_reap_leases", json!({}), &reply).await?;
    assert!(none.is_error());
    assert!(none.sent.is_empty());

    let dry = call(
        "sandbox_reap_leases",
        json!({"older_than_seconds": 60, "dry_run": true}),
        &reply,
    )
    .await?;
    assert!(!dry.is_error());
    let body = dry.sent.first().ok_or("nothing was sent")?.json()?;
    assert_eq!(body["dry_run"], json!(true));
    Ok(())
}

#[tokio::test]
async fn capabilities_can_be_filtered_by_provider() -> TestResult {
    let reply = ListCapabilitiesResponse { profiles: Vec::new() };
    let all = call("sandbox_list_capabilities", json!({"provider": null}), &reply).await?;
    assert!(!all.is_error());
    let mock = call("sandbox_list_capabilities", json!({"provider": "mock"}), &reply).await?;
    assert_eq!(
        mock.sent
            .first()
            .map(|s| s.json())
            .transpose()?
            .map(|b| b["provider"].clone()),
        Some(json!("mock"))
    );
    let bad = call("sandbox_list_capabilities", json!({"provider": "nowhere"}), &reply).await?;
    assert!(bad.is_error());
    assert!(bad.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn extending_a_timeout_returns_the_new_deadline() -> TestResult {
    let reply = ExtendLeaseTimeoutResponse::Extended {
        new_deadline: SystemTime::UNIX_EPOCH + Duration::from_secs(9),
    };
    let outcome = call(
        "sandbox_extend_timeout",
        json!({"lease_id": "l1", "additional_seconds": 30}),
        &reply,
    )
    .await?;
    assert!(!outcome.is_error());
    let body = outcome.sent.first().ok_or("nothing was sent")?.json()?;
    assert_eq!(body["additional_seconds"], json!(30));
    assert!(outcome.text()?.contains("new_deadline"));
    Ok(())
}

#[tokio::test]
async fn usage_and_limits_are_read_for_the_tenant() -> TestResult {
    let used = call(
        "sandbox_usage",
        json!({"lease_id": "l1"}),
        &GetLeaseUsageResponse::Usage { usage: usage()? },
    )
    .await?;
    assert!(!used.is_error());
    assert!(used.text()?.contains("commands_run"));

    let limits = AccountLimits {
        active_leases: 1,
        max_active_leases: 5,
        cpu_seconds_used: 0,
        cpu_seconds_cap: 0,
        wall_seconds_used: 0,
        wall_seconds_cap: 0,
        max_cpu_seconds_per_request: 0,
        max_memory_mib_per_request: 0,
        per_tag: None,
    };
    let reply = GetAccountLimitsResponse::Limits { limits };
    let outcome = call("sandbox_limits", json!({}), &reply).await?;
    assert!(!outcome.is_error());
    assert_eq!(
        outcome
            .sent
            .first()
            .map(|s| s.json())
            .transpose()?
            .map(|b| b["tenant"].clone()),
        Some(Value::from("acme"))
    );
    Ok(())
}
