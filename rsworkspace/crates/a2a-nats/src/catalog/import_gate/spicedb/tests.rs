use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use authzed::v1::check_bulk_permissions_pair;
use authzed::v1::check_permission_response::Permissionship;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsResponse, CheckBulkPermissionsResponseItem, ZedToken,
};
use tonic::Status;
use trogon_std::env::InMemoryEnv;

use crate::agent_id::A2aAgentId;
use crate::catalog::import_gate::gate::ImportGate;
use crate::catalog::import_gate::principal::{ImportedAccountName, SpiceDbPrincipal};
use crate::catalog::import_gate::spicedb::config::{ENV_SPICEDB_ENDPOINT, ENV_SPICEDB_TOKEN, ENV_SPICEDB_ZEDTOKEN_TTL_SECS};

use super::cache::{ImportGateCacheKey, ZedTokenCache};
use super::client::BulkImportPermissionCheck;
use super::config::ZedTokenTtl;
use super::SpiceDbImportGate;

struct MockBulkImportClient {
    responses: Mutex<Vec<Result<CheckBulkPermissionsResponse, Status>>>,
    requests: Mutex<Vec<CheckBulkPermissionsRequest>>,
}

impl MockBulkImportClient {
    fn new(responses: Vec<Result<CheckBulkPermissionsResponse, Status>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn recorded_requests(&self) -> Vec<CheckBulkPermissionsRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl BulkImportPermissionCheck for MockBulkImportClient {
    async fn check_bulk_permissions(
        &self,
        request: CheckBulkPermissionsRequest,
    ) -> Result<CheckBulkPermissionsResponse, Status> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Err(Status::unavailable("no mock response queued")))
    }
}

fn principal(subject: &str) -> SpiceDbPrincipal {
    SpiceDbPrincipal::new(subject)
}

fn imported(name: &str) -> ImportedAccountName {
    ImportedAccountName::new(name)
}

fn agent_id(name: &str) -> A2aAgentId {
    A2aAgentId::new(name).unwrap()
}

fn allowed_response(token: &str) -> CheckBulkPermissionsResponse {
    CheckBulkPermissionsResponse {
        pairs: vec![authzed::v1::CheckBulkPermissionsPair {
            request: None,
            response: Some(check_bulk_permissions_pair::Response::Item(
                CheckBulkPermissionsResponseItem {
                    permissionship: Permissionship::HasPermission as i32,
                    partial_caveat_info: None,
                    debug_trace: None,
                },
            )),
        }],
        checked_at: Some(ZedToken {
            token: token.to_owned(),
        }),
    }
}

fn denied_response() -> CheckBulkPermissionsResponse {
    CheckBulkPermissionsResponse {
        pairs: vec![authzed::v1::CheckBulkPermissionsPair {
            request: None,
            response: Some(check_bulk_permissions_pair::Response::Item(
                CheckBulkPermissionsResponseItem {
                    permissionship: Permissionship::NoPermission as i32,
                    partial_caveat_info: None,
                    debug_trace: None,
                },
            )),
        }],
        checked_at: None,
    }
}

#[tokio::test]
async fn unconfigured_gate_denies() {
    let gate = SpiceDbImportGate::deny_only();
    let allowed = gate
        .permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
        .await
        .expect("deny-only gate must not error");
    assert!(!allowed);
    assert!(!gate.is_configured());
}

#[tokio::test]
async fn try_from_env_without_knobs_builds_deny_only_gate() {
    let env = InMemoryEnv::new();
    let gate = SpiceDbImportGate::try_from_env(&env)
        .await
        .expect("unset env should succeed with deny-only gate");
    assert!(!gate.is_configured());
    assert!(
        !gate
            .permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
            .await
            .expect("must not error")
    );
}

#[tokio::test]
async fn configured_gate_allows_and_caches_zed_token() {
    let mock = Arc::new(MockBulkImportClient::new(vec![
        Ok(allowed_response("zed-1")),
        Ok(allowed_response("zed-2")),
    ]));
    let gate = SpiceDbImportGate::configured(mock.clone(), ZedTokenCache::new(ZedTokenTtl::from_secs(30)));

    assert!(
        gate
            .permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
            .await
            .expect("allowed response")
    );

    let requests = mock.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].items[0].resource.as_ref().unwrap().object_type,
        "agent_card"
    );
    assert_eq!(
        requests[0].items[0].resource.as_ref().unwrap().object_id,
        "peer:bot"
    );
    assert_eq!(requests[0].items[0].permission, "view");

    assert!(
        gate
            .permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
            .await
            .expect("second allowed response")
    );
    assert_eq!(mock.recorded_requests().len(), 2);
    assert!(mock.recorded_requests()[1]
        .consistency
        .as_ref()
        .is_some_and(|consistency| consistency.requirement.is_some()));
}

#[tokio::test]
async fn configured_gate_denies_without_permission() {
    let mock = Arc::new(MockBulkImportClient::new(vec![Ok(denied_response())]));
    let gate = SpiceDbImportGate::configured(mock, ZedTokenCache::new(ZedTokenTtl::from_secs(30)));

    assert!(
        !gate
            .permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
            .await
            .expect("denied response must not error")
    );
}

#[tokio::test]
async fn configured_gate_denies_on_transport_error() {
    let mock = Arc::new(MockBulkImportClient::new(vec![Err(Status::unavailable(
        "spicedb down",
    ))]));
    let gate = SpiceDbImportGate::configured(mock, ZedTokenCache::new(ZedTokenTtl::from_secs(30)));

    assert!(
        !gate
            .permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
            .await
            .expect("transport errors must fail closed")
    );
}

#[tokio::test]
async fn principal_account_claim_is_used_as_import_subject() {
    let mock = Arc::new(MockBulkImportClient::new(vec![Ok(allowed_response("zed-account"))]));
    let gate = SpiceDbImportGate::configured(mock.clone(), ZedTokenCache::new(ZedTokenTtl::from_secs(30)));
    let principal = SpiceDbPrincipal(serde_json::json!({"account": "consumer-acct"}));

    assert!(gate.permit(&principal, &imported("peer"), &agent_id("bot")).await.expect("allowed"));

    let requests = mock.recorded_requests();
    let subject = requests[0].items[0]
        .subject
        .as_ref()
        .unwrap()
        .object
        .as_ref()
        .unwrap();
    assert_eq!(subject.object_type, "account");
    assert_eq!(subject.object_id, "consumer-acct");
}

#[tokio::test]
async fn partial_env_configuration_is_rejected() {
    let env = InMemoryEnv::new();
    env.set(ENV_SPICEDB_ENDPOINT, "https://spicedb.example.com:443");

    match SpiceDbImportGate::try_from_env(&env).await {
        Err(error) => assert!(error.to_string().contains(ENV_SPICEDB_TOKEN)),
        Ok(_) => panic!("token must be required with endpoint"),
    }
}

#[test]
fn zed_token_cache_evicts_expired_entries() {
    let cache = ZedTokenCache::new(ZedTokenTtl::from_secs(1));
    let key = ImportGateCacheKey::new(&imported("peer"), &agent_id("bot"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    rt.block_on(async {
        cache.insert(key.clone(), "zed-old".into()).await;
        assert!(cache.get(&key).await.is_some());
    });

    std::thread::sleep(std::time::Duration::from_millis(1_100));

    rt.block_on(async {
        assert!(cache.get(&key).await.is_none());
    });
}

#[tokio::test]
async fn resolve_import_gate_honors_zedtoken_ttl_env() {
    let env = InMemoryEnv::new();
    env.set(ENV_SPICEDB_ZEDTOKEN_TTL_SECS, "45");

    let gate = SpiceDbImportGate::try_from_env(&env)
        .await
        .expect("ttl-only env should build deny-only gate");
    assert!(!gate.is_configured());
}
