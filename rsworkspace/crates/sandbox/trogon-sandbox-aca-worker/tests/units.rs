use std::num::NonZeroU32;
use std::time::Duration;

use clap::Parser;
use trogon_nats::AdvancedMockNatsClient;
use trogon_sandbox_aca_worker::{
    AcaError, AzureVolumeBackends, BrokerCredentialIssuer, CredentialIssuer, ServicePrincipal, SessionIdentifier,
    SessionStatus, VolumeBackends, VolumeCredential, WorkerArgs, build_heartbeat, credential_ttl_seconds,
    extract_cancel_lease_id,
};
use trogon_sandbox_contract::{
    CapabilitySupport, CredentialIssueOutcome, CredentialIssueResponse, CredentialProvider, CredentialScope,
    LeaseError, LeaseId, ProviderKind, PullSecretId, ShortLivedToken, TenantId, TokenId, WorkerId,
};
use trogon_sandbox_volumes::{StoreResourceName, VolumeBackend};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn ttl(value: u32) -> Result<NonZeroU32, Box<dyn std::error::Error>> {
    Ok(NonZeroU32::new(value).ok_or("zero")?)
}

#[test]
fn a_lease_ttl_is_capped_at_the_maximum_lifetime() -> TestResult {
    assert_eq!(credential_ttl_seconds(60, false, ttl(300)?), ttl(60)?);
    assert_eq!(credential_ttl_seconds(900, false, ttl(300)?), ttl(300)?);
    Ok(())
}

#[test]
fn a_zero_ttl_counts_as_one_second_and_no_ttl_uses_the_maximum() -> TestResult {
    assert_eq!(credential_ttl_seconds(0, false, ttl(300)?), ttl(1)?);
    assert_eq!(credential_ttl_seconds(0, true, ttl(300)?), ttl(300)?);
    Ok(())
}

#[test]
fn session_statuses_ignore_case() {
    for ready in ["Ready", "RUNNING", "active", "Allocated", "succeeded"] {
        assert_eq!(SessionStatus::parse(ready), SessionStatus::Ready, "{ready}");
    }
    assert_eq!(
        SessionStatus::parse("Creating"),
        SessionStatus::Pending("Creating".to_owned())
    );
}

#[test]
fn a_session_body_names_its_status_in_properties_first() {
    assert!(!SessionStatus::from_body(r#"{"properties":{"status":"Creating"},"status":"Ready"}"#).is_ready());
    assert!(SessionStatus::from_body(r#"{"properties":{},"status":"Ready"}"#).is_ready());
    assert!(!SessionStatus::from_body(r#"{"status":"Creating"}"#).is_ready());
}

#[test]
fn a_body_that_names_no_status_describes_a_ready_session() {
    assert!(SessionStatus::from_body("{}").is_ready());
    assert!(SessionStatus::from_body("").is_ready());
    assert!(SessionStatus::from_body("not json").is_ready());
}

#[test]
fn generated_session_identifiers_are_unique_and_printable() {
    let first = SessionIdentifier::generate();
    let second = SessionIdentifier::generate();

    assert_ne!(first, second);
    assert!(first.as_str().starts_with("aca-"));
    assert_eq!(first.to_string(), first.as_str());
    assert_eq!(SessionIdentifier::new("fixed").as_str(), "fixed");
}

#[test]
fn a_service_principal_needs_exactly_three_parts() -> TestResult {
    let principal = ServicePrincipal::parse(&ShortLivedToken::new("client:secret:tenant"))?;

    assert_eq!(principal.client_id, "client");
    assert_eq!(principal.client_secret.expose_secret(), "secret");
    assert_eq!(principal.tenant_id, "tenant");
    for malformed in ["client:secret", "a:b:c:d", ""] {
        assert!(matches!(
            ServicePrincipal::parse(&ShortLivedToken::new(malformed)),
            Err(AcaError::Auth(_))
        ));
    }
    Ok(())
}

#[test]
fn a_volume_credential_takes_an_account_and_a_key() -> TestResult {
    let plain = VolumeCredential::parse(&ShortLivedToken::new("account:key"))?;
    let with_session = VolumeCredential::parse(&ShortLivedToken::new("account:key:session"))?;

    assert_eq!(plain.account, "account");
    assert_eq!(plain.key.expose_secret(), "key");
    assert_eq!(with_session.key.expose_secret(), "key");
    assert!(matches!(
        VolumeCredential::parse(&ShortLivedToken::new("account")),
        Err(AcaError::VolumeCredentialFormat)
    ));
    Ok(())
}

#[test]
fn azure_volumes_live_in_blob_containers_of_the_account() -> TestResult {
    let credential = VolumeCredential::parse(&ShortLivedToken::new("account:key"))?;

    let backend = AzureVolumeBackends.backend(
        StoreResourceName::new("box")?,
        StoreResourceName::new("account")?,
        &credential,
    );

    assert!(matches!(backend, VolumeBackend::AzureBlob { .. }));
    Ok(())
}

#[test]
fn the_error_a_caller_sees_names_what_went_wrong() {
    assert!(matches!(
        AcaError::InvalidSpec("bad".to_owned()).into_lease_error(),
        LeaseError::InvalidSpec { reason } if reason == "bad"
    ));
    assert!(matches!(
        AcaError::SnapshotUnsupported("no".to_owned()).into_lease_error(),
        LeaseError::SnapshotUnsupported { provider: ProviderKind::AzureContainerApps, reason } if reason == "no"
    ));
    assert!(matches!(
        AcaError::aca_api(500, "down").into_lease_error(),
        LeaseError::Backend { message } if message.contains("500") && message.contains("down")
    ));
}

#[test]
fn a_cleanup_failure_names_both_errors() -> TestResult {
    let error = AcaError::AllocationCleanup {
        primary: Box::new(AcaError::aca_api(500, "first")),
        cleanup: Box::new(AcaError::aca_api(503, "second")),
    };

    let AcaError::AllocationCleanup { primary, cleanup } = error else {
        return Err("not a cleanup failure".into());
    };

    assert!(matches!(*primary, AcaError::AcaApi { status: 500, .. }));
    assert!(matches!(*cleanup, AcaError::AcaApi { status: 503, .. }));
    Ok(())
}

#[test]
fn a_cancel_subject_names_its_lease_in_the_last_segment() {
    assert_eq!(
        extract_cancel_lease_id("sandbox.provider.azure_container_apps.cancel.lease-1"),
        Some(LeaseId::new("lease-1"))
    );
    assert_eq!(extract_cancel_lease_id("sandbox.cancel."), None);
    assert_eq!(extract_cancel_lease_id(""), None);
}

#[test]
fn a_heartbeat_advertises_the_region_when_there_is_one() -> TestResult {
    let worker = WorkerId::new("worker-1")?;

    let placed = build_heartbeat(&worker, 4, 1, Some("westeurope"));
    let unplaced = build_heartbeat(&worker, 4, 0, None);

    assert_eq!(placed.worker_id, worker);
    assert_eq!(placed.provider, ProviderKind::AzureContainerApps);
    assert_eq!(placed.capacity.max_concurrent_leases, 4);
    assert_eq!(placed.capacity.current_leases, 1);
    assert_eq!(placed.supports_lease_epoch, CapabilitySupport::Unsupported);
    assert_eq!(placed.profile.placement.regions, vec!["westeurope".to_owned()]);
    assert!(unplaced.profile.placement.regions.is_empty());
    Ok(())
}

fn args(extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let mut argv = vec![
        "worker",
        "--pool-management-endpoint",
        "https://pool.example/management",
        "--session-pool-name",
        "pool",
    ];
    argv.extend_from_slice(extra);
    WorkerArgs::try_parse_from(argv)
}

#[test]
fn the_defaults_point_at_a_local_server_and_the_public_login_authority() -> TestResult {
    let args = args(&[])?;

    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert_eq!(args.max_concurrent_leases, 4);
    assert_eq!(args.max_lease_lifetime_seconds.get(), 86_400);
    assert_eq!(args.azure_credential_id, "azure-sp");
    assert_eq!(args.volume_credential_id, "azure-volume");
    assert!(args.dispatch_fence_file.is_none());
    assert!(args.default_image.is_none());
    assert!(args.region.is_none());
    assert_eq!(args.login_endpoint.as_str(), "https://login.microsoftonline.com/");
    Ok(())
}

#[test]
fn the_provider_config_carries_the_azure_side_of_the_flags() -> TestResult {
    let args = args(&[
        "--default-image",
        "registry.example/app:3",
        "--volume-storage-account",
        "account",
        "--azure-credential-id",
        "sp-1",
        "--volume-credential-id",
        "vol-1",
        "--region",
        "westeurope",
    ])?;

    let config = args.provider_config();

    assert_eq!(config.management_endpoint.as_str(), "https://pool.example/management");
    assert_eq!(config.session_pool_name, "pool");
    assert_eq!(
        config.default_image.map(|image| image.allowlist_string()),
        Some("registry.example/app:3".to_owned())
    );
    assert_eq!(config.azure_credential_id, PullSecretId::new("sp-1"));
    assert_eq!(config.volume_credential_id, PullSecretId::new("vol-1"));
    assert_eq!(
        config.volume_storage_account.map(|account| account.as_str().to_owned()),
        Some("account".to_owned())
    );
    assert_eq!(args.region.as_deref(), Some("westeurope"));
    Ok(())
}

#[test]
fn the_pool_must_be_named() {
    assert!(WorkerArgs::try_parse_from(["worker"]).is_err());
}

#[tokio::test]
async fn the_broker_issues_the_token_it_is_asked_for() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(
        "sandbox.credentials.issue.azure.acme",
        bytes::Bytes::from(serde_json::to_vec(&CredentialIssueOutcome::Ok(
            CredentialIssueResponse {
                token_id: TokenId::new("tok-1"),
                token: ShortLivedToken::new("client:secret:tenant"),
                expires_at: std::time::UNIX_EPOCH + Duration::from_secs(100),
            },
        ))?),
    );

    let token = BrokerCredentialIssuer::new(mock.clone())
        .issue(
            CredentialProvider::Azure,
            &TenantId::new("acme")?,
            &PullSecretId::new("azure-sp"),
            CredentialScope::AzureServicePrincipal {
                resource_id: "resource".to_owned(),
            },
            ttl(60)?,
        )
        .await?;

    assert_eq!(token.expose_secret(), "client:secret:tenant");
    assert_eq!(mock.requested_payloads().len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_broker_that_does_not_answer_is_a_credential_error() -> TestResult {
    let error = BrokerCredentialIssuer::new(AdvancedMockNatsClient::new())
        .issue(
            CredentialProvider::Azure,
            &TenantId::new("acme")?,
            &PullSecretId::new("azure-sp"),
            CredentialScope::AzureServicePrincipal {
                resource_id: "resource".to_owned(),
            },
            ttl(60)?,
        )
        .await
        .err();

    assert!(matches!(error, Some(AcaError::Credential(_))));
    Ok(())
}
