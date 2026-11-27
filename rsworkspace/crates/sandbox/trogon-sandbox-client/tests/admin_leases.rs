mod admin_support;
mod common;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use admin_support::{tenant, unauthorized};
use common::{MockBus, TestResult, client_replying, lease_id};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    GetLeaseByNameResponse, GetSandboxDomainResponse, LeaseCreation, LeaseError, LeaseGranted, LeaseId, LeaseName,
    LeaseResponse, ListCapabilitiesResponse, ListLeasesRequest, ListLeasesResponse, NativeHandleResponse, ProviderKind,
    ReapLeasesRequest, ReapLeasesResponse, ReapMode, SandboxTags, SshAccessResponse,
};
use trogon_sandbox_protocol::subjects;

fn granted() -> LeaseGranted {
    LeaseGranted {
        lease_id: LeaseId::new("lease-fork"),
        provider: ProviderKind::Mock,
        sandbox_id: "sandbox-1".into(),
        evidence_subject: "sandbox.evidence.lease-fork.>".into(),
        deadline: SystemTime::now() + Duration::from_secs(300),
        created: LeaseCreation::Created,
        capabilities: Vec::new(),
        entity_id: None,
        operation_id: None,
    }
}

#[tokio::test]
async fn fork_returns_the_granted_lease() -> TestResult {
    let granted = granted();
    let (client, log) = client_replying(&LeaseResponse::Granted(granted.clone()))?;

    let forked = client
        .fork(tenant()?, &LeaseId::new("lease-source"), 300, "fork-key-1".into())
        .await?;

    assert_eq!(forked.lease_id, granted.lease_id);
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::LEASE_FORK_REQUEST);
    assert_eq!(sent.timeout, Duration::from_secs(30));
    assert_eq!(sent.json()?["idempotency_key"], "fork-key-1");
    Ok(())
}

#[tokio::test]
async fn fork_returns_denied_when_the_broker_denies() -> TestResult {
    let (client, _) = client_replying(&LeaseResponse::Denied(LeaseError::SourceLeaseNotRunning {
        lease_id: LeaseId::new("lease-source"),
    }))?;

    let result = client
        .fork(tenant()?, &LeaseId::new("lease-source"), 300, "k".into())
        .await;

    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::SourceLeaseNotRunning { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn get_sandbox_domain_returns_the_answer_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&GetSandboxDomainResponse::PortNotExposed)?;
    let got = client.get_sandbox_domain(&lease_id(), 8080).await?;
    assert_eq!(got, GetSandboxDomainResponse::PortNotExposed);
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::lease_get_domain(&lease_id()));
    assert_eq!(sent.json()?["port"], 8080);

    let (client, _) = client_replying(&GetSandboxDomainResponse::Denied(unauthorized()))?;
    let denied = client.get_sandbox_domain(&lease_id(), 8080).await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

#[tokio::test]
async fn native_handle_returns_the_answer_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&NativeHandleResponse::Unsupported)?;
    let got = client.native_handle(&lease_id()).await?;
    assert_eq!(got, NativeHandleResponse::Unsupported);
    assert_eq!(log.only()?.subject, subjects::lease_native_handle(&lease_id()));

    let (client, _) = client_replying(&NativeHandleResponse::Denied(unauthorized()))?;
    let denied = client.native_handle(&lease_id()).await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

#[tokio::test]
async fn ssh_access_returns_the_answer_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&SshAccessResponse::Unsupported)?;
    let got = client.ssh_access(&lease_id(), Some(60)).await?;
    assert_eq!(got, SshAccessResponse::Unsupported);
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::lease_ssh_access(&lease_id()));
    assert_eq!(sent.json()?["ttl_seconds"], 60);

    let (client, _) = client_replying(&SshAccessResponse::Denied(unauthorized()))?;
    let denied = client.ssh_access(&lease_id(), None).await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

#[tokio::test]
async fn get_lease_by_name_returns_the_answer_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&GetLeaseByNameResponse::NotFound)?;
    let got = client.get_lease_by_name(tenant()?, LeaseName::new("web")?).await?;
    assert_eq!(got, GetLeaseByNameResponse::NotFound);
    assert_eq!(log.only()?.subject, subjects::LEASE_GET_BY_NAME_REQUEST);

    let (client, _) = client_replying(&GetLeaseByNameResponse::Denied(unauthorized()))?;
    let denied = client.get_lease_by_name(tenant()?, LeaseName::new("web")?).await;
    assert!(matches!(denied, Err(ClientError::Denied(_))), "{denied:?}");
    Ok(())
}

fn list_request() -> Result<ListLeasesRequest, Box<dyn std::error::Error>> {
    Ok(ListLeasesRequest {
        tenant: tenant()?,
        status_filter: None,
        cursor: None,
        limit: None,
        tags: SandboxTags::default(),
    })
}

#[tokio::test]
async fn list_leases_returns_the_page() -> TestResult {
    let (client, log) = client_replying(&ListLeasesResponse {
        leases: Vec::new(),
        next_cursor: Some("next".into()),
        denied: None,
    })?;

    let page = client.list_leases(list_request()?).await?;

    assert_eq!(page.next_cursor.as_deref(), Some("next"));
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::LEASE_LIST_REQUEST);
    assert_eq!(sent.timeout, Duration::from_secs(2));
    Ok(())
}

#[tokio::test]
async fn list_leases_surfaces_a_refusal_as_denied_not_as_empty() -> TestResult {
    let (client, _) = client_replying(&ListLeasesResponse {
        leases: Vec::new(),
        next_cursor: None,
        denied: Some(unauthorized()),
    })?;

    let leases = client.list_leases(list_request()?).await;

    assert!(matches!(leases, Err(ClientError::Denied(_))), "{leases:?}");
    Ok(())
}

fn reap_request() -> Result<ReapLeasesRequest, Box<dyn std::error::Error>> {
    let tags = SandboxTags::new(BTreeMap::from([("app".to_string(), "web".to_string())]))?;
    Ok(ReapLeasesRequest::new(
        tenant()?,
        tags,
        Some(3600),
        None,
        ReapMode::DryRun,
    )?)
}

#[tokio::test]
async fn reap_leases_sends_the_request_on_the_reap_subject_and_parses_the_reply() -> TestResult {
    let reply = ReapLeasesResponse::Reaped {
        dry_run: ReapMode::DryRun,
        matched: vec![],
        released: vec![],
        failed: vec![],
    };
    let (client, log) = client_replying(&reply)?;
    let request = reap_request()?;

    let response = client.reap_leases(request.clone()).await?;

    assert_eq!(response, reply);
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::LEASE_REAP_REQUEST);
    assert_eq!(sent.timeout, Duration::from_secs(30));
    assert_eq!(serde_json::from_slice::<ReapLeasesRequest>(&sent.payload)?, request);
    Ok(())
}

#[tokio::test]
async fn reap_leases_surfaces_denial() -> TestResult {
    let (client, _) = client_replying(&ReapLeasesResponse::Denied(unauthorized()))?;

    let result = client.reap_leases(reap_request()?).await;

    assert!(matches!(result, Err(ClientError::Denied(_))), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn list_capabilities_returns_the_profiles_and_sends_the_filter() -> TestResult {
    let (client, log) = client_replying(&ListCapabilitiesResponse::default())?;

    let profiles = client.list_capabilities(Some(ProviderKind::Mock)).await?;

    assert!(profiles.is_empty());
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::CAPABILITY_LIST_REQUEST);
    assert_eq!(sent.json()?["provider"], "mock");
    Ok(())
}

#[tokio::test]
async fn lease_calls_time_out_when_the_bus_does_not_reply() -> TestResult {
    let result = MockBus::timing_out().into_client().list_capabilities(None).await;

    assert!(matches!(result, Err(ClientError::Timeout(_))));
    Ok(())
}
