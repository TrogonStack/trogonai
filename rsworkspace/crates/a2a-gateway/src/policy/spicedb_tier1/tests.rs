use std::sync::Mutex;

use a2a_nats::catalog::import_gate::ZedTokenTtl;
use a2a_pack::Tier1Permission;
use authzed::v1::{
    CheckBulkPermissionsPair, CheckBulkPermissionsRequest, CheckBulkPermissionsResponse,
    CheckBulkPermissionsResponseItem, ZedToken, check_bulk_permissions_pair, check_permission_response::Permissionship,
};
use trogon_std::env::{InMemoryEnv, ReadEnv};

use super::*;

fn agent(s: &str) -> A2aAgentId {
    A2aAgentId::new(s).expect("nats-safe test agent")
}

fn empty_principal() -> SpiceDbPrincipal {
    SpiceDbPrincipal(serde_json::json!({}))
}

fn principal_with_subject(subject: &str) -> SpiceDbPrincipal {
    SpiceDbPrincipal(serde_json::json!({ "spicedb_subject": subject }))
}

struct FakeBulkClient {
    responses: Mutex<Vec<i32>>,
    transport_err: Mutex<bool>,
    requests: Mutex<Vec<CheckBulkPermissionsRequest>>,
    write_requests: Mutex<Vec<WriteRelationshipsRequest>>,
}

impl FakeBulkClient {
    fn new(responses: Vec<i32>) -> Self {
        Self {
            responses: Mutex::new(responses),
            transport_err: Mutex::new(false),
            requests: Mutex::new(Vec::new()),
            write_requests: Mutex::new(Vec::new()),
        }
    }

    fn with_transport_error() -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            transport_err: Mutex::new(true),
            requests: Mutex::new(Vec::new()),
            write_requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<CheckBulkPermissionsRequest> {
        self.requests.lock().expect("lock").clone()
    }

    fn writes(&self) -> Vec<WriteRelationshipsRequest> {
        self.write_requests.lock().expect("lock").clone()
    }
}

#[async_trait]
impl BulkImportPermissionCheck for FakeBulkClient {
    async fn check_bulk_permissions(
        &self,
        request: CheckBulkPermissionsRequest,
    ) -> Result<tonic::Response<CheckBulkPermissionsResponse>, Status> {
        self.requests.lock().expect("lock").push(request.clone());
        if *self.transport_err.lock().expect("lock") {
            return Err(Status::unavailable("backend"));
        }
        let perms = self.responses.lock().expect("lock").clone();
        let pairs = request
            .items
            .into_iter()
            .enumerate()
            .map(|(idx, _)| {
                let permissionship = perms.get(idx).copied().unwrap_or(Permissionship::HasPermission as i32);
                CheckBulkPermissionsPair {
                    request: None,
                    response: Some(check_bulk_permissions_pair::Response::Item(
                        CheckBulkPermissionsResponseItem {
                            permissionship,
                            partial_caveat_info: None,
                            debug_trace: None,
                        },
                    )),
                }
            })
            .collect();
        Ok(tonic::Response::new(CheckBulkPermissionsResponse {
            checked_at: Some(ZedToken { token: "zed".into() }),
            pairs,
        }))
    }

    async fn write_relationships(
        &self,
        request: WriteRelationshipsRequest,
    ) -> Result<tonic::Response<authzed::v1::WriteRelationshipsResponse>, Status> {
        self.write_requests.lock().expect("lock").push(request);
        Ok(tonic::Response::new(authzed::v1::WriteRelationshipsResponse {
            written_at: None,
        }))
    }
}

#[tokio::test]
async fn noop_gate_is_disabled_and_authorizes_everything() {
    let gate = NoopSpiceDbTier1Gate;
    assert!(!gate.is_enabled());
    let outcome = gate
        .authorize(
            &Tier1SessionKey::new("alice", "acme"),
            &empty_principal(),
            &Tier1ResourceTuple::new(
                Tier1ResourceType::new("agent").expect("valid"),
                Tier1ResourceId::new("planner").expect("valid"),
                Tier1Permission::new("invoke").expect("valid"),
            ),
        )
        .await;
    assert_eq!(outcome, Tier1AuthorizeOutcome::Allowed { zed_token: None });
}

#[tokio::test]
async fn noop_gate_bulk_check_returns_allowed_for_every_agent() {
    let gate = NoopSpiceDbTier1Gate;
    let outcomes = gate
        .bulk_check_agent_view(
            &Tier1SessionKey::new("alice", "acme"),
            &empty_principal(),
            &[agent("a1"), agent("a2"), agent("a3")],
        )
        .await;
    assert_eq!(outcomes.len(), 3);
    assert!(outcomes.iter().all(|o| *o == AgentViewCheckOutcome::Allowed));
}

#[test]
fn tier1_owner_tuple_for_task_builds_validated_value_objects() {
    let owner = Tier1OwnerTuple::for_task(&agent("planner"), "task-1", "user", "alice").expect("valid");
    assert_eq!(owner.resource_type.as_str(), "task");
    assert_eq!(owner.resource_id.as_str(), "planner:task-1");
    assert_eq!(owner.relation, "owner");
}

#[test]
fn derive_tuple_handles_each_a2a_method() {
    let principal_agent = agent("planner");
    let tuple = derive_tuple(
        &A2aMethod::MessageSend,
        &principal_agent,
        "publisher",
        &serde_json::json!({}),
    )
    .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "invoke");
    assert_eq!(tuple.resource_type.as_str(), "agent");

    let tuple = derive_tuple(
        &A2aMethod::TasksGet,
        &principal_agent,
        "publisher",
        &serde_json::json!({"id": "task-1"}),
    )
    .expect("derive ok");
    assert_eq!(tuple.permission.as_str(), "read");
    assert_eq!(tuple.resource_id.as_str(), "planner:task-1");
}

#[test]
fn derive_tuple_surfaces_missing_task_id() {
    let err = derive_tuple(
        &A2aMethod::TasksGet,
        &agent("planner"),
        "publisher",
        &serde_json::json!({}),
    )
    .expect_err("missing task id must error");
    assert_eq!(err, Tier1DeriveError::MissingTaskId);
}

#[tokio::test]
async fn from_env_returns_noop_when_disabled() {
    let env = InMemoryEnv::new();
    let layer = Tier1SpiceDbConfig::from_env(&env).await.expect("noop ok");
    assert!(!layer.gate.is_enabled());
    assert!(layer.owner_emitter.is_none());
    assert!(!layer.discovery_view.is_enabled());
}

#[tokio::test]
async fn from_env_errors_when_enabled_without_credentials() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "true");
    let err = Tier1SpiceDbConfig::from_env(&env)
        .await
        .expect_err("must require credentials");
    assert!(matches!(err, Tier1SpiceDbBuildError::PartialConfig(_)));
}

#[tokio::test]
async fn from_env_errors_when_endpoint_set_without_token() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "true");
    env.set(ENV_TIER1_SPICEDB_ENDPOINT, "https://spicedb.example/");
    let err = Tier1SpiceDbConfig::from_env(&env)
        .await
        .expect_err("must require token");
    assert!(matches!(err, Tier1SpiceDbBuildError::PartialConfig(_)));
}

#[tokio::test]
async fn from_env_errors_when_token_set_without_endpoint() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "true");
    env.set(ENV_TIER1_SPICEDB_TOKEN, "tok");
    let err = Tier1SpiceDbConfig::from_env(&env)
        .await
        .expect_err("must require endpoint");
    assert!(matches!(err, Tier1SpiceDbBuildError::PartialConfig(_)));
}

#[tokio::test]
async fn from_env_rejects_invalid_ttl() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_ZEDTOKEN_TTL_SECS, "not-a-number");
    let err = Tier1SpiceDbConfig::from_env(&env).await.expect_err("must reject");
    assert!(matches!(err, Tier1SpiceDbBuildError::InvalidZedTokenTtl(_)));
}

#[tokio::test]
async fn from_env_dials_when_fully_configured() {
    // Fully configured env now dials the SpiceDB endpoint. A
    // localhost port that refuses connections produces `Connect`
    // -- we just need to verify the path goes through the dial
    // rather than returning a placeholder stub.
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "true");
    env.set(ENV_TIER1_SPICEDB_ENDPOINT, "http://127.0.0.1:1");
    env.set(ENV_TIER1_SPICEDB_TOKEN, "tok");
    let err = Tier1SpiceDbConfig::from_env(&env)
        .await
        .expect_err("unreachable must error");
    assert!(matches!(err, Tier1SpiceDbBuildError::Connect(_)));
}

#[tokio::test]
async fn live_gate_denies_when_principal_lacks_subject_claim() {
    let client = Arc::new(FakeBulkClient::new(vec![]));
    let gate = LiveSpiceDbTier1Gate::new(
        client.clone(),
        SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)),
    );
    let session = Tier1SessionKey::new("alice", "acme");
    let outcome = gate
        .authorize(
            &session,
            &empty_principal(),
            &Tier1ResourceTuple::new(
                Tier1ResourceType::new("agent").expect("valid"),
                Tier1ResourceId::new("planner").expect("valid"),
                Tier1Permission::new("invoke").expect("valid"),
            ),
        )
        .await;
    assert_eq!(outcome, Tier1AuthorizeOutcome::Denied);
    assert!(client.requests().is_empty(), "must short-circuit without RPC");
}

#[tokio::test]
async fn live_gate_allows_when_pair_has_permission() {
    let client = Arc::new(FakeBulkClient::new(vec![Permissionship::HasPermission as i32]));
    let gate = LiveSpiceDbTier1Gate::new(
        client.clone(),
        SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)),
    );
    let session = Tier1SessionKey::new("alice", "acme");
    let outcome = gate
        .authorize(
            &session,
            &principal_with_subject("user:alice"),
            &Tier1ResourceTuple::new(
                Tier1ResourceType::new("agent").expect("valid"),
                Tier1ResourceId::new("planner").expect("valid"),
                Tier1Permission::new("invoke").expect("valid"),
            ),
        )
        .await;
    assert!(matches!(outcome, Tier1AuthorizeOutcome::Allowed { zed_token: Some(_) }));
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn live_gate_denies_when_pair_has_no_permission() {
    let client = Arc::new(FakeBulkClient::new(vec![Permissionship::NoPermission as i32]));
    let gate = LiveSpiceDbTier1Gate::new(client, SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
    let outcome = gate
        .authorize(
            &Tier1SessionKey::new("alice", "acme"),
            &principal_with_subject("user:alice"),
            &Tier1ResourceTuple::new(
                Tier1ResourceType::new("agent").expect("valid"),
                Tier1ResourceId::new("planner").expect("valid"),
                Tier1Permission::new("invoke").expect("valid"),
            ),
        )
        .await;
    assert_eq!(outcome, Tier1AuthorizeOutcome::Denied);
}

#[tokio::test]
async fn live_gate_transport_error_surfaces_distinctly() {
    let client = Arc::new(FakeBulkClient::with_transport_error());
    let gate = LiveSpiceDbTier1Gate::new(client, SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
    let outcome = gate
        .authorize(
            &Tier1SessionKey::new("alice", "acme"),
            &principal_with_subject("user:alice"),
            &Tier1ResourceTuple::new(
                Tier1ResourceType::new("agent").expect("valid"),
                Tier1ResourceId::new("planner").expect("valid"),
                Tier1Permission::new("invoke").expect("valid"),
            ),
        )
        .await;
    assert_eq!(outcome, Tier1AuthorizeOutcome::TransportError);
}

#[tokio::test]
async fn live_gate_caches_zed_token_after_allowed_response() {
    // First successful response seeds the cache; the second
    // request must then carry `AtLeastAsFresh` consistency.
    let client = Arc::new(FakeBulkClient::new(vec![
        Permissionship::HasPermission as i32,
        Permissionship::HasPermission as i32,
    ]));
    let gate = LiveSpiceDbTier1Gate::new(
        client.clone(),
        SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)),
    );
    let session = Tier1SessionKey::new("alice", "acme");
    let tuple = Tier1ResourceTuple::new(
        Tier1ResourceType::new("agent").expect("valid"),
        Tier1ResourceId::new("planner").expect("valid"),
        Tier1Permission::new("invoke").expect("valid"),
    );
    gate.authorize(&session, &principal_with_subject("user:alice"), &tuple)
        .await;
    gate.authorize(&session, &principal_with_subject("user:alice"), &tuple)
        .await;

    let requests = client.requests();
    assert_eq!(requests.len(), 2);
    let second_consistency = requests[1].consistency.as_ref().expect("consistency");
    assert!(
        matches!(
            second_consistency.requirement,
            Some(authzed::v1::consistency::Requirement::AtLeastAsFresh(_))
        ),
        "second request must reuse cached zed token via AtLeastAsFresh",
    );
}

#[tokio::test]
async fn live_gate_emit_owner_writes_relationship() {
    let client = Arc::new(FakeBulkClient::new(vec![]));
    let gate = LiveSpiceDbTier1Gate::new(
        client.clone(),
        SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)),
    );
    let owner = Tier1OwnerTuple::for_task(&agent("planner"), "task-1", "user", "alice").expect("valid");
    gate.emit_owner(&owner).await.expect("write ok");
    assert_eq!(client.writes().len(), 1);
}

#[tokio::test]
async fn live_gate_bulk_check_agent_view_routes_through_view_gate() {
    let client = Arc::new(FakeBulkClient::new(vec![Permissionship::HasPermission as i32]));
    let gate = LiveSpiceDbTier1Gate::new(client, SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
    let outcomes = gate
        .bulk_check_agent_view(
            &Tier1SessionKey::new("alice", "acme"),
            &principal_with_subject("user:alice"),
            &[agent("a1")],
        )
        .await;
    assert_eq!(outcomes, vec![AgentViewCheckOutcome::Allowed]);
}

#[tokio::test]
async fn live_gate_is_enabled() {
    let client = Arc::new(FakeBulkClient::new(vec![]));
    let gate = LiveSpiceDbTier1Gate::new(client, SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
    assert!(gate.is_enabled());
}

#[test]
fn tier1_principal_from_caller_carries_subject_and_account() {
    let principal = tier1_principal_from_caller("alice", "acme");
    assert_eq!(
        principal.0.get("spicedb_subject").and_then(serde_json::Value::as_str),
        Some("user/alice"),
    );
    assert_eq!(
        principal.0.get("sub").and_then(serde_json::Value::as_str),
        Some("alice"),
    );
    assert_eq!(
        principal.0.get("session_account").and_then(serde_json::Value::as_str),
        Some("acme"),
    );
}

#[test]
fn tier1_session_from_principal_extracts_session_key() {
    let principal = tier1_principal_from_caller("alice", "acme");
    let session = tier1_session_from_principal(&principal, "fallback").expect("present");
    assert_eq!(session.sub(), "alice");
    assert_eq!(session.account(), "acme");
}

#[test]
fn tier1_enabled_recognizes_padded_truthy_values() {
    for raw in ["1", "true", " on ", "\tyes\n"] {
        let env = InMemoryEnv::new();
        env.set(ENV_TIER1_SPICEDB_ENABLED, raw);
        assert!(tier1_enabled(&env), "{raw:?} must enable");
    }
    let env = InMemoryEnv::new();
    assert!(!tier1_enabled(&env));
}

#[test]
fn tier1_enabled_handles_non_unicode_env_as_disabled() {
    let env = AllNotUnicodeEnv;
    assert!(!tier1_enabled(&env));
}

struct AllNotUnicodeEnv;
impl ReadEnv for AllNotUnicodeEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotUnicode(std::ffi::OsString::from("garbage")))
    }
}

#[test]
fn tier1_zed_token_ttl_rejects_non_unicode_env() {
    let env = AllNotUnicodeEnv;
    let err = tier1_zed_token_ttl(&env).expect_err("non-unicode must reject");
    assert!(matches!(err, Tier1SpiceDbBuildError::InvalidZedTokenTtl(_)));
}

struct PartiallyNotUnicodeEnv {
    ok: (&'static str, &'static str),
}
impl ReadEnv for PartiallyNotUnicodeEnv {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        if key == self.ok.0 {
            Ok(self.ok.1.into())
        } else {
            Err(std::env::VarError::NotUnicode(std::ffi::OsString::from("garbage")))
        }
    }
}

#[test]
fn optional_tier1_credentials_rejects_non_unicode_endpoint() {
    let env = PartiallyNotUnicodeEnv {
        ok: (ENV_TIER1_SPICEDB_TOKEN, "tok"),
    };
    let err = optional_tier1_credentials(&env).expect_err("non-unicode endpoint must reject");
    assert!(matches!(err, Tier1SpiceDbBuildError::InvalidEndpoint(_)));
}

#[test]
fn optional_tier1_credentials_rejects_non_unicode_token() {
    let env = PartiallyNotUnicodeEnv {
        ok: (ENV_TIER1_SPICEDB_ENDPOINT, "https://spicedb.example"),
    };
    let err = optional_tier1_credentials(&env).expect_err("non-unicode token must reject");
    assert!(matches!(err, Tier1SpiceDbBuildError::InvalidToken(_)));
}

#[test]
fn gateway_tier1_layer_debug_emits_summary_fields() {
    let layer = GatewayTier1Layer::noop();
    let debug = format!("{layer:?}");
    assert!(debug.contains("gate_is_enabled"));
    assert!(debug.contains("has_owner_emitter"));
    assert!(debug.contains("discovery_view_is_enabled"));
}

#[test]
fn gateway_tier1_layer_noop_has_disabled_gates() {
    let layer = GatewayTier1Layer::noop();
    assert!(!layer.gate.is_enabled());
    assert!(layer.owner_emitter.is_none());
    assert!(!layer.discovery_view.is_enabled());
}

#[test]
fn spicedb_import_gate_build_error_lifts_into_tier1_error() {
    let from = Tier1SpiceDbBuildError::from(SpiceDbImportGateBuildError::InvalidEndpoint("bad".into()));
    assert!(matches!(from, Tier1SpiceDbBuildError::InvalidEndpoint(_)));
    let from = Tier1SpiceDbBuildError::from(SpiceDbImportGateBuildError::InvalidToken("bad".into()));
    assert!(matches!(from, Tier1SpiceDbBuildError::InvalidToken(_)));
    let from = Tier1SpiceDbBuildError::from(SpiceDbImportGateBuildError::InvalidZedTokenTtl("bad".into()));
    assert!(matches!(from, Tier1SpiceDbBuildError::InvalidZedTokenTtl(_)));
    let from = Tier1SpiceDbBuildError::from(SpiceDbImportGateBuildError::Connect("bad".into()));
    assert!(matches!(from, Tier1SpiceDbBuildError::Connect(_)));
}

#[test]
fn tier1_authorize_outcome_variants_compare_by_value() {
    assert_eq!(
        Tier1AuthorizeOutcome::Allowed { zed_token: None },
        Tier1AuthorizeOutcome::Allowed { zed_token: None },
    );
    assert_ne!(
        Tier1AuthorizeOutcome::Allowed { zed_token: None },
        Tier1AuthorizeOutcome::Denied
    );
    assert_ne!(Tier1AuthorizeOutcome::Denied, Tier1AuthorizeOutcome::TransportError);
    assert_ne!(
        Tier1AuthorizeOutcome::TransportError,
        Tier1AuthorizeOutcome::DeriveFailed
    );
}
