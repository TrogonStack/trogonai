use trogon_sandbox_auth::{ApiKeyStore, Authorizer, AuthzError, SandboxAuthzRequest};
use trogon_sandbox_contract::{CancelLeaseResponse, LeaseError, LeaseId, ServiceAccountId, TenantId};

use std::sync::Arc;
use std::time::SystemTime;

use async_nats::HeaderMap;
use async_trait::async_trait;
use trogon_sandbox_auth::{ApiKeyRecord, BearerToken, InMemoryApiKeyStore, NoopAuthorizer};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct MockDenyAuthorizer;

#[async_trait]
impl Authorizer for MockDenyAuthorizer {
    async fn authorize_sandbox_lease(&self, _req: &SandboxAuthzRequest) -> Result<(), AuthzError> {
        Err(AuthzError::Denied)
    }
}

fn tenant(name: &str) -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new(name)?)
}

fn lease_id(name: &str) -> LeaseId {
    LeaseId::new(name)
}

fn make_record(name: &str) -> Result<ApiKeyRecord, Box<dyn std::error::Error>> {
    Ok(ApiKeyRecord::new(
        tenant(name)?,
        ServiceAccountId::new("sa-1"),
        "test-key",
        SystemTime::now(),
    ))
}

async fn store_with_key(raw_token: &str, tenant: &str) -> Result<Arc<InMemoryApiKeyStore>, Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryApiKeyStore::new());
    store
        .insert(BearerToken::new(raw_token)?.hash(), make_record(tenant)?)
        .await;
    Ok(store)
}

fn headers_with_bearer(token: &str) -> HeaderMap {
    let mut map = HeaderMap::new();
    map.insert("Authorization", format!("Bearer {token}").as_str());
    map
}

fn ctx_with(store: Arc<dyn ApiKeyStore>, authz: Arc<dyn Authorizer>, enforce: bool) -> AuthContext {
    AuthContext {
        api_keys: store,
        authorizer: authz,
        enforce,
    }
}

#[tokio::test]
async fn happy_path_returns_allowed() -> TestResult {
    let store = store_with_key("good-token", "tenant-a").await?;
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let headers = headers_with_bearer("good-token");
    let tenant = tenant("tenant-a")?;

    let decision = authenticate_lease_request(&ctx, Some(&headers), &tenant).await;
    assert!(matches!(decision, AuthDecision::Allowed { .. }));
    Ok(())
}

#[tokio::test]
async fn missing_header_returns_denied() -> TestResult {
    let store = Arc::new(InMemoryApiKeyStore::new());
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let tenant = tenant("tenant-a")?;

    let decision = authenticate_lease_request(&ctx, None, &tenant).await;
    assert!(matches!(
        decision,
        AuthDecision::Denied(AuthDenialReason::MissingHeader)
    ));
    Ok(())
}

#[tokio::test]
async fn non_bearer_scheme_returns_missing_header() -> TestResult {
    let store = Arc::new(InMemoryApiKeyStore::new());
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let tenant = tenant("tenant-a")?;

    let mut headers = HeaderMap::new();
    headers.insert("Authorization", "Basic dXNlcjpwYXNz");
    let decision = authenticate_lease_request(&ctx, Some(&headers), &tenant).await;
    assert!(matches!(
        decision,
        AuthDecision::Denied(AuthDenialReason::MissingHeader)
    ));
    Ok(())
}

#[tokio::test]
async fn unknown_key_returns_denied() -> TestResult {
    let store = Arc::new(InMemoryApiKeyStore::new());
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let tenant = tenant("tenant-a")?;

    let headers = headers_with_bearer("no-such-key");
    let decision = authenticate_lease_request(&ctx, Some(&headers), &tenant).await;
    assert!(matches!(decision, AuthDecision::Denied(AuthDenialReason::UnknownKey)));
    Ok(())
}

#[tokio::test]
async fn tenant_mismatch_returns_denied() -> TestResult {
    let store = store_with_key("good-token", "tenant-a").await?;
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let tenant = tenant("tenant-b")?;

    let headers = headers_with_bearer("good-token");
    let decision = authenticate_lease_request(&ctx, Some(&headers), &tenant).await;
    assert!(matches!(
        decision,
        AuthDecision::Denied(AuthDenialReason::TenantMismatch)
    ));
    Ok(())
}

#[tokio::test]
async fn permission_denied_by_authorizer_returns_denied() -> TestResult {
    let store = store_with_key("good-token", "tenant-a").await?;
    let ctx = ctx_with(store, Arc::new(MockDenyAuthorizer), true);
    let tenant = tenant("tenant-a")?;

    let headers = headers_with_bearer("good-token");
    let decision = authenticate_lease_request(&ctx, Some(&headers), &tenant).await;
    assert!(matches!(
        decision,
        AuthDecision::Denied(AuthDenialReason::PermissionDenied)
    ));
    Ok(())
}

#[tokio::test]
async fn enforce_false_bypasses_auth_and_returns_allowed() -> TestResult {
    let store = Arc::new(InMemoryApiKeyStore::new());
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), false);
    let tenant = tenant("tenant-a")?;

    let decision = authenticate_lease_request(&ctx, None, &tenant).await;
    assert!(matches!(decision, AuthDecision::Allowed { .. }));
    Ok(())
}

#[tokio::test]
async fn every_gated_verb_denies_a_wrong_tenant_key_when_enforcing() -> TestResult {
    let store = store_with_key("tenant-b-token", "tenant-b").await?;
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let headers = headers_with_bearer("tenant-b-token");
    let victim = tenant("tenant-a")?;
    let own_tenant = tenant("tenant-b")?;
    let lease = lease_id("lease-1");

    for &verb in ClientVerb::ALL {
        match verb.scope() {
            VerbScope::Lease => {
                let denied = authorize_lease(&ctx, Some(&headers), verb, &lease, async { Some(victim.clone()) }).await;
                assert!(
                    matches!(denied, Err(LeaseError::Unauthorized { .. })),
                    "{verb:?}: {denied:?}"
                );
                let unauthenticated = authorize_lease(&ctx, None, verb, &lease, async { Some(victim.clone()) }).await;
                assert!(
                    matches!(unauthenticated, Err(LeaseError::Unauthorized { .. })),
                    "{verb:?}: {unauthenticated:?}"
                );
                let unknown = authorize_lease(&ctx, Some(&headers), verb, &lease, async { None }).await;
                assert!(
                    matches!(unknown, Err(LeaseError::LeaseNotFound { .. })),
                    "{verb:?}: {unknown:?}"
                );
                let own = authorize_lease(&ctx, Some(&headers), verb, &lease, async { Some(own_tenant.clone()) }).await;
                assert_eq!(own, Ok(()), "{verb:?}");
            }
            VerbScope::Tenant => {
                let denied = authorize_tenant(&ctx, Some(&headers), verb, &victim).await;
                assert!(denied.is_err(), "{verb:?}");
                let unauthenticated = authorize_tenant(&ctx, None, verb, &victim).await;
                assert!(unauthenticated.is_err(), "{verb:?}");
                let own = authorize_tenant(&ctx, Some(&headers), verb, &own_tenant).await;
                assert_eq!(own, Ok(()), "{verb:?}");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn every_gated_verb_allows_everything_when_not_enforcing() -> TestResult {
    let store = Arc::new(InMemoryApiKeyStore::new());
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), false);
    let tenant = tenant("tenant-a")?;
    let lease = lease_id("lease-1");

    for &verb in ClientVerb::ALL {
        match verb.scope() {
            VerbScope::Lease => {
                let polled = std::sync::atomic::AtomicBool::new(false);
                let allowed = authorize_lease(&ctx, None, verb, &lease, async {
                    polled.store(true, std::sync::atomic::Ordering::SeqCst);
                    None
                })
                .await;
                assert_eq!(allowed, Ok(()), "{verb:?}");
                assert!(
                    !polled.load(std::sync::atomic::Ordering::SeqCst),
                    "{verb:?} looked up the owner"
                );
            }
            VerbScope::Tenant => {
                assert_eq!(authorize_tenant(&ctx, None, verb, &tenant).await, Ok(()), "{verb:?}");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_refused_cancel_is_answered_and_never_runs() -> TestResult {
    let store = store_with_key("tenant-b-token", "tenant-b").await?;
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let headers = headers_with_bearer("tenant-b-token");
    let lease = lease_id("lease-1");
    let ran = std::sync::atomic::AtomicBool::new(false);
    let owner = tenant("tenant-a")?;

    let response = cancel_lease_for_client(&ctx, Some(&headers), &lease, async { Some(owner) }, || async {
        ran.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok::<(), std::io::Error>(())
    })
    .await;

    assert!(
        matches!(response, CancelLeaseResponse::Denied(LeaseError::Unauthorized { .. })),
        "{response:?}"
    );
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn an_authorized_cancel_reports_its_outcome() -> TestResult {
    let store = store_with_key("tenant-a-token", "tenant-a").await?;
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let headers = headers_with_bearer("tenant-a-token");
    let lease = lease_id("lease-1");
    let owner_tenant = tenant("tenant-a")?;
    let owner = || async { Some(owner_tenant.clone()) };

    let done = cancel_lease_for_client(&ctx, Some(&headers), &lease, owner(), || async {
        Ok::<(), std::io::Error>(())
    })
    .await;
    assert_eq!(done, CancelLeaseResponse::Cancelled);

    let failed = cancel_lease_for_client(&ctx, Some(&headers), &lease, owner(), || async {
        Err(std::io::Error::other("store unavailable"))
    })
    .await;
    assert!(
        matches!(failed, CancelLeaseResponse::Denied(LeaseError::Backend { .. })),
        "{failed:?}"
    );
    Ok(())
}

/// Subjects the broker subscribes to that carry no client verb, and why.
const UNGATED_SUBSCRIPTIONS: &[&str] = &[
    // Worker-to-broker events, not client requests.
    "WORKER_LEASE_ACTIVITY_FILTER",
    // A worker asking which of its own leases are still assigned to it.
    // Workers carry no verifiable credential, so the answer discloses only
    // the leases the request lists and the broker records for that worker.
    "WORKER_AUTHORITY_FILTER",
    "SNAPSHOT_PUBLISHED",
    "SNAPSHOT_DELETED",
    // Internal accountant queue: answers snapshot accounting between broker
    // replicas, never a client request.
    "SNAPSHOT_ACCOUNTING_REQUEST",
    // Lists provider capabilities; carries no tenant data.
    "CAPABILITY_LIST_REQUEST",
];

/// Every client subject the broker subscribes to, by the constant
/// `main.rs` passes to `subscribe`, and the verb that gates it.
const CLIENT_SUBSCRIPTIONS: &[(&str, ClientVerb)] = &[
    ("LEASE_REQUEST", ClientVerb::LeaseRequest),
    ("LEASE_FORK_REQUEST", ClientVerb::LeaseFork),
    ("LEASE_CANCEL_FILTER", ClientVerb::Cancel),
    ("LEASE_TAGS_UPDATE_FILTER", ClientVerb::TagsUpdate),
    ("LEASE_NETWORK_POLICY_UPDATE_FILTER", ClientVerb::NetworkPolicyUpdate),
    ("LEASE_EXTEND_TIMEOUT_FILTER", ClientVerb::ExtendTimeout),
    ("LEASE_USAGE_FILTER", ClientVerb::LeaseUsage),
    ("TENANT_USAGE_REQUEST", ClientVerb::TenantUsage),
    ("TENANT_LIMITS_REQUEST", ClientVerb::TenantLimits),
    ("LEASE_DELETE_FILTER", ClientVerb::DeleteLease),
    ("LEASE_STOP_FILTER", ClientVerb::StopLease),
    ("LEASE_PAUSE_FILTER", ClientVerb::PauseLease),
    ("LEASE_RESUME_FILTER", ClientVerb::ResumeLease),
    ("LEASE_SUSPEND_FILTER", ClientVerb::SuspendLease),
    ("LEASE_WAKE_FILTER", ClientVerb::WakeLease),
    ("LEASE_ROLLBACK_FILTER", ClientVerb::RollbackLease),
    ("LEASE_SNAPSHOT_FILTER", ClientVerb::SnapshotLease),
    ("SNAPSHOT_DELETE_REQUEST", ClientVerb::SnapshotDelete),
    ("NATIVE_SNAPSHOT_DELETE_REQUEST", ClientVerb::NativeSnapshotDelete),
    ("LEASE_RUN_COMMAND_FILTER", ClientVerb::RunCommand),
    ("LEASE_POLL_COMMAND_FILTER", ClientVerb::PollCommand),
    ("LEASE_KILL_COMMAND_FILTER", ClientVerb::KillCommand),
    ("LEASE_CANCEL_COMMAND_LAUNCH_FILTER", ClientVerb::CancelCommandLaunch),
    ("LEASE_STOP_COMMAND_FILTER", ClientVerb::StopCommand),
    ("LEASE_RESTART_COMMAND_FILTER", ClientVerb::RestartCommand),
    ("LEASE_READ_LOGS_FILTER", ClientVerb::ReadLogs),
    ("LEASE_WRITE_STDIN_FILTER", ClientVerb::WriteStdin),
    ("LEASE_LIST_PROCESSES_FILTER", ClientVerb::ListProcesses),
    ("LEASE_WAIT_READY_FILTER", ClientVerb::WaitReady),
    ("LEASE_WAIT_SANDBOX_READY_FILTER", ClientVerb::WaitSandboxReady),
    ("LEASE_GET_DOMAIN_FILTER", ClientVerb::GetDomain),
    ("LEASE_TERMINAL_FILTER", ClientVerb::Terminal),
    ("LEASE_NATIVE_HANDLE_FILTER", ClientVerb::NativeHandle),
    ("LEASE_SSH_ACCESS_FILTER", ClientVerb::SshAccess),
    ("LEASE_WRITE_FILES_FILTER", ClientVerb::WriteFiles),
    ("LEASE_READ_FILE_FILTER", ClientVerb::ReadFile),
    ("LEASE_FILE_OP_FILTER", ClientVerb::FileOp),
    ("LEASE_CREATE_USER_FILTER", ClientVerb::CreateUser),
    ("LEASE_CREATE_GROUP_FILTER", ClientVerb::CreateGroup),
    (
        "LEASE_MODIFY_GROUP_MEMBERSHIP_FILTER",
        ClientVerb::ModifyGroupMembership,
    ),
    ("DRIVE_CREATE_REQUEST", ClientVerb::DriveCreate),
    ("DRIVE_LIST_REQUEST", ClientVerb::DriveList),
    ("DRIVE_DELETE_REQUEST", ClientVerb::DriveDelete),
    ("LEASE_LIST_REQUEST", ClientVerb::LeaseList),
    ("LEASE_REAP_REQUEST", ClientVerb::LeaseReap),
    ("LEASE_GET_BY_NAME_REQUEST", ClientVerb::GetLeaseByName),
    ("SNAPSHOT_GET_REQUEST", ClientVerb::SnapshotGet),
    ("SNAPSHOT_TREE_REQUEST", ClientVerb::SnapshotTree),
    ("SNAPSHOT_TAG_SET_REQUEST", ClientVerb::SnapshotTagSet),
    ("SNAPSHOT_TAG_DELETE_REQUEST", ClientVerb::SnapshotTagDelete),
    ("SNAPSHOT_TAG_HISTORY_REQUEST", ClientVerb::SnapshotTagHistory),
    ("DELETE_NAMED_SANDBOX_REQUEST", ClientVerb::DeleteNamed),
    ("OPERATION_GET_REQUEST", ClientVerb::OperationGet),
    ("OPERATION_CANCEL_REQUEST", ClientVerb::OperationCancel),
    ("OPERATION_CAPABILITIES_REQUEST", ClientVerb::OperationCapabilities),
    ("ENTITY_GET_REQUEST", ClientVerb::GetEntity),
    ("ENTITY_LIST_REQUEST", ClientVerb::ListEntities),
    ("ENTITY_DELETE_REQUEST", ClientVerb::DeleteEntity),
];

/// The subject constants the serve route tables pass to the router.
fn broker_subscriptions() -> Vec<&'static str> {
    [
        include_str!("../serve/tenant_routes.rs"),
        include_str!("../serve/lease_routes.rs"),
        include_str!("../serve/event_routes.rs"),
    ]
    .into_iter()
    .flat_map(|source| {
        source.split(".").skip(1).filter_map(|rest| {
            let (kind, args) = rest.split_once('(')?;
            matches!(kind.trim(), "tenant" | "lease" | "raw" | "queue").then_some(())?;
            args.split(',').next().map(str::trim)
        })
    })
    .collect()
}

#[test]
fn every_broker_subscription_maps_to_a_client_verb() -> TestResult {
    let subscriptions = broker_subscriptions();
    assert!(!subscriptions.is_empty());
    for subject in &subscriptions {
        assert!(
            CLIENT_SUBSCRIPTIONS.iter().any(|(s, _)| s == subject) || UNGATED_SUBSCRIPTIONS.contains(subject),
            "{subject} is subscribed without a ClientVerb; add it to CLIENT_SUBSCRIPTIONS or UNGATED_SUBSCRIPTIONS"
        );
    }
    for (subject, verb) in CLIENT_SUBSCRIPTIONS {
        assert!(
            subscriptions.contains(subject),
            "{subject} ({verb:?}) is no longer subscribed"
        );
    }
    for verb in ClientVerb::ALL {
        let subjects = CLIENT_SUBSCRIPTIONS.iter().filter(|(_, v)| v == verb).count();
        assert_eq!(subjects, 1, "{verb:?} gates {subjects} subscriptions");
    }
    assert_eq!(CLIENT_SUBSCRIPTIONS.len(), ClientVerb::ALL.len());
    Ok(())
}

#[test]
fn a_file_request_must_name_the_lease_its_subject_authorized() -> TestResult {
    let lease = lease_id("lease-1");
    assert_eq!(require_subject_lease(&lease, &lease), Ok(()));
    assert!(matches!(
        require_subject_lease(&lease, &lease_id("lease-2")),
        Err(LeaseError::Unauthorized { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn reading_an_operation_needs_a_key_for_the_operations_tenant() -> TestResult {
    let store = store_with_key("tenant-b-token", "tenant-b").await?;
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let headers = headers_with_bearer("tenant-b-token");
    let other = tenant("tenant-a")?;
    let own = tenant("tenant-b")?;

    assert_eq!(ClientVerb::OperationGet.scope(), VerbScope::Tenant);
    assert!(
        authorize_tenant(&ctx, Some(&headers), ClientVerb::OperationGet, &other)
            .await
            .is_err()
    );
    assert!(
        authorize_tenant(&ctx, None, ClientVerb::OperationGet, &own)
            .await
            .is_err()
    );
    assert_eq!(
        authorize_tenant(&ctx, Some(&headers), ClientVerb::OperationGet, &own).await,
        Ok(())
    );
    Ok(())
}

#[tokio::test]
async fn entity_verbs_need_a_key_for_the_requested_tenant() -> TestResult {
    let store = store_with_key("tenant-b-token", "tenant-b").await?;
    let ctx = ctx_with(store, Arc::new(NoopAuthorizer), true);
    let headers = headers_with_bearer("tenant-b-token");
    let other = tenant("tenant-a")?;
    let own = tenant("tenant-b")?;

    for verb in [
        ClientVerb::GetEntity,
        ClientVerb::ListEntities,
        ClientVerb::DeleteEntity,
    ] {
        assert_eq!(verb.scope(), VerbScope::Tenant);
        assert!(
            authorize_tenant(&ctx, Some(&headers), verb, &other).await.is_err(),
            "{verb:?} accepted another tenant's key"
        );
        assert!(
            authorize_tenant(&ctx, None, verb, &own).await.is_err(),
            "{verb:?} accepted a request with no key"
        );
        assert_eq!(authorize_tenant(&ctx, Some(&headers), verb, &own).await, Ok(()));
    }
    Ok(())
}

#[test]
fn the_verb_table_has_no_duplicates() -> TestResult {
    for (i, a) in ClientVerb::ALL.iter().enumerate() {
        assert!(!ClientVerb::ALL[i + 1..].contains(a), "{a:?} listed twice");
    }
    Ok(())
}
