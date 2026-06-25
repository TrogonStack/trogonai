use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use authzed::v1::check_bulk_permissions_pair;
use authzed::v1::check_permission_response::Permissionship;
use authzed::v1::{
    CheckBulkPermissionsPair, CheckBulkPermissionsRequest, CheckBulkPermissionsResponse,
    CheckBulkPermissionsResponseItem, WriteRelationshipsRequest, WriteRelationshipsResponse, ZedToken,
};
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use tonic::Status;
use trogon_std::env::InMemoryEnv;

use crate::agent_id::A2aAgentId;
use crate::catalog::import_gate::gate::ImportGate;
use crate::catalog::import_gate::principal::{ImportedAccountName, SpiceDbPrincipal};
use crate::catalog::import_gate::spicedb::config::{
    ENV_SPICEDB_ENDPOINT, ENV_SPICEDB_TOKEN, ENV_SPICEDB_ZEDTOKEN_TTL_SECS,
};

use super::cache::{ImportGateCacheKey, ZedTokenCache};
use super::client::BulkImportPermissionCheck;
use super::config::{SpiceDbEndpoint, SpiceDbImportGateBuildError, SpiceDbToken, ZedTokenTtl};
use super::config::{optional_spicedb_credentials, zed_token_ttl_from_env};
use super::{SpiceDbImportGate, parse_subject_reference, spicedb_subject_from_principal};

struct MockBulkImportClient {
    responses: Mutex<Vec<Result<CheckBulkPermissionsResponse, Status>>>,
    write_responses: Mutex<Vec<Result<WriteRelationshipsResponse, Status>>>,
    requests: Mutex<Vec<CheckBulkPermissionsRequest>>,
    write_requests: Mutex<Vec<WriteRelationshipsRequest>>,
}

impl MockBulkImportClient {
    fn new(responses: Vec<Result<CheckBulkPermissionsResponse, Status>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            write_responses: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            write_requests: Mutex::new(Vec::new()),
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

    async fn write_relationships(
        &self,
        request: WriteRelationshipsRequest,
    ) -> Result<WriteRelationshipsResponse, Status> {
        self.write_requests.lock().unwrap().push(request);
        self.write_responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Ok(WriteRelationshipsResponse { written_at: None }))
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

#[test]
fn optional_credentials_unset_env_returns_none() {
    let env = InMemoryEnv::new();
    assert!(optional_spicedb_credentials(&env).unwrap().is_none());
}

#[tokio::test]
async fn configured_gate_allows_and_caches_zed_token() {
    let mock = Arc::new(MockBulkImportClient::new(vec![
        Ok(allowed_response("zed-1")),
        Ok(allowed_response("zed-2")),
    ]));
    let gate = SpiceDbImportGate::configured(mock.clone(), ZedTokenCache::new(ZedTokenTtl::from_secs(30)));

    assert!(
        gate.permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
            .await
            .expect("allowed response")
    );

    let requests = mock.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].items[0].resource.as_ref().unwrap().object_type,
        "agent_card"
    );
    assert_eq!(requests[0].items[0].resource.as_ref().unwrap().object_id, "peer:bot");
    assert_eq!(requests[0].items[0].permission, "view");

    assert!(
        gate.permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
            .await
            .expect("second allowed response")
    );
    assert_eq!(mock.recorded_requests().len(), 2);
    assert!(
        mock.recorded_requests()[1]
            .consistency
            .as_ref()
            .is_some_and(|consistency| consistency.requirement.is_some())
    );
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

    assert!(
        gate.permit(&principal, &imported("peer"), &agent_id("bot"))
            .await
            .expect("allowed")
    );

    let requests = mock.recorded_requests();
    let subject = requests[0].items[0].subject.as_ref().unwrap().object.as_ref().unwrap();
    assert_eq!(subject.object_type, "account");
    assert_eq!(subject.object_id, "consumer-acct");
}

#[test]
fn optional_credentials_rejects_partial_configuration() {
    let env = InMemoryEnv::new();
    env.set(ENV_SPICEDB_ENDPOINT, "https://spicedb.example.com:443");
    let err = optional_spicedb_credentials(&env).unwrap_err();
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidToken(_)));
    assert_eq!(
        err.to_string(),
        format!("invalid SpiceDB token: {ENV_SPICEDB_TOKEN} is required when {ENV_SPICEDB_ENDPOINT} is set")
    );
}

#[test]
fn optional_credentials_rejects_partial_configuration_token_only() {
    let env = InMemoryEnv::new();
    env.set(ENV_SPICEDB_TOKEN, "tk");
    let err = optional_spicedb_credentials(&env).unwrap_err();
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidEndpoint(_)));
    assert_eq!(
        err.to_string(),
        format!("invalid SpiceDB endpoint: {ENV_SPICEDB_ENDPOINT} is required when {ENV_SPICEDB_TOKEN} is set")
    );
}

#[test]
fn optional_credentials_returns_parsed_pair_when_both_set() {
    let env = InMemoryEnv::new();
    env.set(ENV_SPICEDB_ENDPOINT, "https://spicedb.example.com:443");
    env.set(ENV_SPICEDB_TOKEN, "tk");
    let (endpoint, token) = optional_spicedb_credentials(&env).unwrap().unwrap();
    assert_eq!(endpoint.as_str(), "https://spicedb.example.com:443");
    assert_eq!(token.expose_secret(), "tk");
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

#[test]
fn zed_token_ttl_from_env_uses_default_when_unset() {
    let env = InMemoryEnv::new();
    let ttl = zed_token_ttl_from_env(&env).unwrap();
    assert_eq!(ttl.as_duration(), std::time::Duration::from_secs(30));
}

#[test]
fn zed_token_ttl_from_env_parses_valid_value() {
    let env = InMemoryEnv::new();
    env.set(ENV_SPICEDB_ZEDTOKEN_TTL_SECS, "45");
    let ttl = zed_token_ttl_from_env(&env).unwrap();
    assert_eq!(ttl.as_duration(), std::time::Duration::from_secs(45));
}

#[test]
fn zed_token_ttl_from_env_rejects_non_numeric_value() {
    let env = InMemoryEnv::new();
    env.set(ENV_SPICEDB_ZEDTOKEN_TTL_SECS, "abc");
    let err = zed_token_ttl_from_env(&env).unwrap_err();
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidZedTokenTtl(_)));
    assert_eq!(err.to_string(), "invalid SpiceDB ZedToken TTL: abc");
}

#[test]
fn endpoint_parse_rejects_blank_string() {
    let err = SpiceDbEndpoint::parse("   ").unwrap_err();
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidEndpoint(_)));
}

#[test]
fn token_parse_rejects_blank_string() {
    let err = SpiceDbToken::parse("   ").unwrap_err();
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidToken(_)));
}

#[test]
fn token_debug_redacts_secret() {
    let token = SpiceDbToken::parse("super-secret").unwrap();
    assert_eq!(format!("{token:?}"), "SpiceDbToken(***)");
}

#[test]
fn build_error_display_covers_every_variant() {
    assert_eq!(
        SpiceDbImportGateBuildError::InvalidEndpoint("e".into()).to_string(),
        "invalid SpiceDB endpoint: e"
    );
    assert_eq!(
        SpiceDbImportGateBuildError::InvalidToken("t".into()).to_string(),
        "invalid SpiceDB token: t"
    );
    assert_eq!(
        SpiceDbImportGateBuildError::InvalidZedTokenTtl("ttl".into()).to_string(),
        "invalid SpiceDB ZedToken TTL: ttl"
    );
    assert_eq!(
        SpiceDbImportGateBuildError::Connect("down".into()).to_string(),
        "SpiceDB connect failed: down"
    );
}

#[test]
fn cache_ttl_accessor_returns_configured_ttl() {
    let cache = ZedTokenCache::new(ZedTokenTtl::from_secs(42));
    assert_eq!(cache.ttl().as_duration(), std::time::Duration::from_secs(42));
}

#[test]
fn parse_subject_reference_accepts_slash_and_colon_separators() {
    assert_eq!(
        parse_subject_reference("user/alice"),
        Some(("user".into(), "alice".into()))
    );
    assert_eq!(
        parse_subject_reference("user:alice"),
        Some(("user".into(), "alice".into()))
    );
}

#[test]
fn parse_subject_reference_rejects_empty_segments() {
    assert_eq!(parse_subject_reference(""), None);
    assert_eq!(parse_subject_reference("user/"), None);
    assert_eq!(parse_subject_reference("/alice"), None);
    assert_eq!(parse_subject_reference("user:"), None);
    assert_eq!(parse_subject_reference(":alice"), None);
    assert_eq!(parse_subject_reference("only-segment"), None);
}

#[test]
fn spicedb_subject_from_principal_prefers_account_then_aud_then_subject() {
    let account = SpiceDbPrincipal(serde_json::json!({"account": "tenant-1"}));
    assert_eq!(
        spicedb_subject_from_principal(&account),
        Some(("account".into(), "tenant-1".into()))
    );

    let aud = SpiceDbPrincipal(serde_json::json!({"aud": "tenant-2"}));
    assert_eq!(
        spicedb_subject_from_principal(&aud),
        Some(("account".into(), "tenant-2".into()))
    );

    let subject = SpiceDbPrincipal(serde_json::json!({"spicedb_subject": "user/alice"}));
    assert_eq!(
        spicedb_subject_from_principal(&subject),
        Some(("user".into(), "alice".into()))
    );
}

#[test]
fn spicedb_subject_from_principal_returns_none_when_no_claim_maps() {
    let principal = SpiceDbPrincipal(serde_json::json!({"sub": "no-match"}));
    assert_eq!(spicedb_subject_from_principal(&principal), None);
}

#[tokio::test]
async fn configured_gate_denies_when_principal_lacks_subject_mapping() {
    let mock = Arc::new(MockBulkImportClient::new(vec![]));
    let gate = SpiceDbImportGate::configured(mock.clone(), ZedTokenCache::new(ZedTokenTtl::from_secs(30)));
    let principal = SpiceDbPrincipal(serde_json::json!({"sub": "no-mapping"}));
    assert!(
        !gate
            .permit(&principal, &imported("peer"), &agent_id("bot"))
            .await
            .expect("must not error")
    );
    assert!(
        mock.recorded_requests().is_empty(),
        "principal without subject must short-circuit before contacting Authzed"
    );
}

#[test]
fn default_returns_deny_only_gate() {
    let gate = SpiceDbImportGate::default();
    assert!(!gate.is_configured());
}

#[tokio::test]
async fn configured_gate_denies_when_per_pair_response_is_absent() {
    let response = CheckBulkPermissionsResponse {
        checked_at: Some(ZedToken {
            token: "zed-empty".into(),
        }),
        pairs: vec![CheckBulkPermissionsPair {
            request: None,
            response: None,
        }],
    };
    let mock = Arc::new(MockBulkImportClient::new(vec![Ok(response)]));
    let gate = SpiceDbImportGate::configured(mock, ZedTokenCache::new(ZedTokenTtl::from_secs(30)));
    assert!(
        !gate
            .permit(&principal("user/alice"), &imported("peer"), &agent_id("bot"))
            .await
            .expect("missing per-pair response must fail closed")
    );
}

/// Env adapter that returns `NotUnicode` for designated keys — `InMemoryEnv`
/// can't simulate this directly because its backing store is `String`-keyed.
#[derive(Default)]
struct NotUnicodeEnv {
    keys: std::collections::HashSet<String>,
}

impl NotUnicodeEnv {
    fn flag(mut self, key: impl Into<String>) -> Self {
        self.keys.insert(key.into());
        self
    }
}

impl trogon_std::env::ReadEnv for NotUnicodeEnv {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        if self.keys.contains(key) {
            #[cfg(unix)]
            {
                Err(std::env::VarError::NotUnicode(std::ffi::OsString::from_vec(vec![
                    0xC3, 0x28,
                ])))
            }
            #[cfg(not(unix))]
            Err(std::env::VarError::NotPresent)
        } else {
            Err(std::env::VarError::NotPresent)
        }
    }
}

#[test]
#[cfg(unix)]
fn zed_token_ttl_from_env_reports_invalid_unicode() {
    let env = NotUnicodeEnv::default().flag(ENV_SPICEDB_ZEDTOKEN_TTL_SECS);
    let err = zed_token_ttl_from_env(&env).unwrap_err();
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidZedTokenTtl(_)));
}

#[test]
#[cfg(unix)]
fn optional_credentials_reports_invalid_unicode_endpoint() {
    let env = NotUnicodeEnv::default().flag(ENV_SPICEDB_ENDPOINT);
    let err = optional_spicedb_credentials(&env).unwrap_err();
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidEndpoint(_)));
}

struct EndpointOkTokenNotUnicodeEnv;

impl trogon_std::env::ReadEnv for EndpointOkTokenNotUnicodeEnv {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        match key {
            ENV_SPICEDB_ENDPOINT => Ok("https://spicedb.example.com:443".into()),
            #[cfg(unix)]
            ENV_SPICEDB_TOKEN => Err(std::env::VarError::NotUnicode(std::ffi::OsString::from_vec(vec![
                0xC3, 0x28,
            ]))),
            _ => Err(std::env::VarError::NotPresent),
        }
    }
}

#[test]
#[cfg(unix)]
fn optional_credentials_reports_invalid_unicode_token() {
    let err = optional_spicedb_credentials(&EndpointOkTokenNotUnicodeEnv).unwrap_err();
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidToken(_)));
}
