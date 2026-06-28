use std::sync::Mutex;

use async_trait::async_trait;
use authzed::v1::{
    CheckBulkPermissionsPair, CheckBulkPermissionsResponse, CheckBulkPermissionsResponseItem, CheckPermissionResponse,
    WriteRelationshipsRequest, WriteRelationshipsResponse, ZedToken, check_bulk_permissions_pair,
    check_permission_response::Permissionship,
};
use trogon_std::env::InMemoryEnv;
use trogon_std::env::ReadEnv;

use super::*;

fn agent(s: &str) -> A2aAgentId {
    A2aAgentId::new(s).expect("nats-safe test agent id")
}

fn principal_with_subject(subject: &str) -> SpiceDbPrincipal {
    SpiceDbPrincipal(serde_json::json!({ "spicedb_subject": subject }))
}

fn principal_with_sub_and_account(sub: &str, account: &str) -> SpiceDbPrincipal {
    SpiceDbPrincipal(serde_json::json!({ "sub": sub, "account": account }))
}

fn empty_principal() -> SpiceDbPrincipal {
    SpiceDbPrincipal(serde_json::json!({}))
}

struct FakeBulkClient {
    /// Permissionship to return for each item; defaults to HasPermission.
    permissionships: Mutex<Vec<i32>>,
    /// `true` once any request has been observed.
    seen: Mutex<bool>,
}

impl FakeBulkClient {
    fn new(permissionships: Vec<i32>) -> Self {
        Self {
            permissionships: Mutex::new(permissionships),
            seen: Mutex::new(false),
        }
    }
}

#[async_trait]
impl BulkImportPermissionCheck for FakeBulkClient {
    async fn check_bulk_permissions(
        &self,
        request: authzed::v1::CheckBulkPermissionsRequest,
    ) -> Result<CheckBulkPermissionsResponse, tonic::Status> {
        *self.seen.lock().expect("seen lock") = true;
        let perms = self.permissionships.lock().expect("perms lock").clone();
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
        Ok(CheckBulkPermissionsResponse {
            checked_at: Some(ZedToken {
                token: "zed-tok".into(),
            }),
            pairs,
        })
    }

    async fn write_relationships(
        &self,
        _request: WriteRelationshipsRequest,
    ) -> Result<WriteRelationshipsResponse, tonic::Status> {
        unimplemented!("agent-view gate doesn't write")
    }
}

// Suppress unused-warning for CheckPermissionResponse import (some
// rustc versions strip the import otherwise).
#[allow(dead_code)]
const _: Option<CheckPermissionResponse> = None;

#[tokio::test]
async fn noop_gate_allows_every_agent() {
    let gate = NoopAgentViewGate;
    let session = SpiceDbSessionKey::new("alice", "acme");
    let principal = principal_with_subject("agent/alice");
    assert!(!gate.is_enabled());
    let outcome = gate.check_agent_view(&session, &principal, &agent("a1")).await;
    assert_eq!(outcome, AgentViewCheckOutcome::Allowed);
}

#[tokio::test]
async fn live_gate_returns_denied_when_principal_lacks_subject_claim() {
    let client = std::sync::Arc::new(FakeBulkClient::new(vec![Permissionship::HasPermission as i32]));
    let gate = LiveAgentViewGate::new(client.clone(), SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60)));
    let session = SpiceDbSessionKey::new("alice", "acme");
    // No `spicedb_subject` claim -- must fail closed without ever
    // touching the gRPC client.
    let principal = empty_principal();
    let outcomes = gate
        .bulk_check_agent_view(&session, &principal, &[agent("a1"), agent("a2")])
        .await;
    assert_eq!(
        outcomes,
        vec![AgentViewCheckOutcome::Denied, AgentViewCheckOutcome::Denied]
    );
    assert!(
        !*client.seen.lock().expect("seen lock"),
        "client must not be called when subject claim missing"
    );
}

#[tokio::test]
async fn live_gate_maps_has_permission_to_allowed() {
    let client = std::sync::Arc::new(FakeBulkClient::new(vec![
        Permissionship::HasPermission as i32,
        Permissionship::NoPermission as i32,
    ]));
    let gate = LiveAgentViewGate::new(client, SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60)));
    let session = SpiceDbSessionKey::new("alice", "acme");
    let principal = principal_with_subject("user:alice");
    let outcomes = gate
        .bulk_check_agent_view(&session, &principal, &[agent("a1"), agent("a2")])
        .await;
    assert_eq!(
        outcomes,
        vec![AgentViewCheckOutcome::Allowed, AgentViewCheckOutcome::Denied]
    );
}

#[tokio::test]
async fn live_gate_check_single_agent_returns_first_outcome() {
    let client = std::sync::Arc::new(FakeBulkClient::new(vec![Permissionship::HasPermission as i32]));
    let gate = LiveAgentViewGate::new(client, SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60)));
    let session = SpiceDbSessionKey::new("alice", "acme");
    let principal = principal_with_subject("user:alice");
    let outcome = gate.check_agent_view(&session, &principal, &agent("a1")).await;
    assert_eq!(outcome, AgentViewCheckOutcome::Allowed);
}

#[tokio::test]
async fn live_gate_bulk_check_on_empty_returns_empty() {
    let client = std::sync::Arc::new(FakeBulkClient::new(vec![]));
    let gate = LiveAgentViewGate::new(client.clone(), SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60)));
    let session = SpiceDbSessionKey::new("alice", "acme");
    let principal = principal_with_subject("user:alice");
    let outcomes = gate.bulk_check_agent_view(&session, &principal, &[]).await;
    assert!(outcomes.is_empty());
    assert!(!*client.seen.lock().expect("seen lock"));
}

#[tokio::test]
async fn session_cache_round_trip_under_ttl() {
    let cache = SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60));
    let key = SpiceDbSessionKey::new("alice", "acme");
    cache.insert(key.clone(), "tok-1".into()).await;
    let snapshot = cache.get(&key).await.expect("cached");
    assert_eq!(snapshot.token, "tok-1");
}

#[tokio::test]
async fn session_cache_returns_fresh_within_ttl() {
    let cache = SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60));
    let key = SpiceDbSessionKey::new("alice", "acme");
    cache.insert(key.clone(), "tok-1".into()).await;
    let snapshot = cache.get(&key).await.expect("cached");
    assert_eq!(snapshot.token, "tok-1");
    // A second read must still hit the cache -- `get` mustn't be
    // self-invalidating when the entry is still fresh.
    assert!(cache.get(&key).await.is_some());
}

#[test]
fn session_from_principal_prefers_sub_claim() {
    let principal = principal_with_sub_and_account("alice", "acme");
    let key = session_from_principal(&principal, "fallback").expect("present");
    assert_eq!(key.sub(), "alice");
    assert_eq!(key.account(), "acme");
}

#[test]
fn session_from_principal_falls_back_when_account_claim_absent() {
    let principal = SpiceDbPrincipal(serde_json::json!({ "sub": "alice" }));
    let key = session_from_principal(&principal, "fallback-account").expect("present");
    assert_eq!(key.account(), "fallback-account");
}

#[test]
fn session_from_principal_returns_none_without_sub_or_subject() {
    let principal = empty_principal();
    assert!(session_from_principal(&principal, "x").is_none());
}

#[test]
fn tier1_enabled_recognizes_truthy_and_padded_values() {
    for raw in ["1", "true", "yes", "on", " true ", "\ttrue\n"] {
        let env = InMemoryEnv::new();
        env.set(ENV_TIER1_SPICEDB_ENABLED, raw);
        assert!(tier1_enabled(&env), "{raw:?} must enable");
    }
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "off");
    assert!(!tier1_enabled(&env));
    let empty = InMemoryEnv::new();
    assert!(!tier1_enabled(&empty));
}

#[test]
fn tier1_zed_token_ttl_uses_env_value_or_default() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_ZEDTOKEN_TTL_SECS, "120");
    let ttl = tier1_zed_token_ttl(&env).expect("parse ok");
    assert_eq!(ttl.as_duration(), std::time::Duration::from_secs(120));

    let empty = InMemoryEnv::new();
    let default = tier1_zed_token_ttl(&empty).expect("default ok");
    assert_eq!(default.as_duration(), std::time::Duration::from_secs(60));
}

/// `ReadEnv` impl that returns `NotUnicode` for any key. `InMemoryEnv`
/// only models `NotPresent`/`Ok`, so this fixture is the only way to
/// hit the `NotUnicode` arm of the env-decode helpers from a test.
struct NotUnicodeEnv;

impl ReadEnv for NotUnicodeEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotUnicode(std::ffi::OsString::from("garbage")))
    }
}

#[test]
fn tier1_zed_token_ttl_rejects_non_unicode_env() {
    let env = NotUnicodeEnv;
    let err = tier1_zed_token_ttl(&env).expect_err("non-unicode env must error");
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidZedTokenTtl(_)));
}

#[test]
fn tier1_zed_token_ttl_rejects_garbage() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_ZEDTOKEN_TTL_SECS, "not-a-number");
    let err = tier1_zed_token_ttl(&env).expect_err("must reject");
    assert!(matches!(err, SpiceDbImportGateBuildError::InvalidZedTokenTtl(_)));
}

#[test]
fn agent_view_gate_layer_noop_constructor() {
    let layer = AgentViewGateLayer::noop();
    assert!(!layer.gate.is_enabled());
}

#[test]
fn agent_view_gate_layer_with_gate_wraps_arbitrary_impl() {
    let gate: Arc<dyn AgentViewGate> = Arc::new(NoopAgentViewGate);
    let layer = AgentViewGateLayer::with_gate(gate.clone());
    assert!(!layer.gate.is_enabled());
}

#[tokio::test]
async fn live_gate_is_enabled() {
    let client = std::sync::Arc::new(FakeBulkClient::new(vec![]));
    let gate = LiveAgentViewGate::new(client, SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60)));
    assert!(gate.is_enabled());
}

#[tokio::test]
async fn session_cache_evicts_stale_entry_on_read() {
    // ZedTokenTtl clamps to a 1-second minimum, so the stale-eviction
    // branch needs at least one second of wall-clock to fire. Worth
    // covering once because losing this branch would make the cache
    // silently serve unbounded-stale tokens.
    let cache = SpiceDbSessionCache::new(ZedTokenTtl::from_secs(1));
    let key = SpiceDbSessionKey::new("alice", "acme");
    cache.insert(key.clone(), "tok".into()).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    assert!(cache.get(&key).await.is_none(), "stale entry must be evicted on read");
    assert!(cache.get(&key).await.is_none());
}

#[tokio::test]
async fn default_bulk_check_falls_back_to_per_agent_fanout() {
    // The default trait impl loops `check_agent_view` per item.
    // Cover it by exercising NoopAgentViewGate's default path
    // (Noop overrides only `check_agent_view`, inherits the
    // default bulk impl).
    let gate = NoopAgentViewGate;
    let session = SpiceDbSessionKey::new("alice", "acme");
    let principal = principal_with_subject("user:alice");
    let outcomes = gate
        .bulk_check_agent_view(&session, &principal, &[agent("a1"), agent("a2"), agent("a3")])
        .await;
    assert_eq!(outcomes.len(), 3);
    assert!(outcomes.iter().all(|o| *o == AgentViewCheckOutcome::Allowed));
}

#[tokio::test]
async fn live_gate_pair_with_no_response_maps_to_denied() {
    // A pair with `response: None` must surface as `Denied`, not
    // TransportError. Covers the `None` arm of `pair_is_allowed`
    // alongside the Item-not-HasPermission case. The Error arm
    // takes a generated `google::rpc::Status` that isn't reachable
    // outside the spicedb-grpc-tonic crate, so the test exercises
    // the `None` neighbour instead -- the two arms share the same
    // `false` mapping in `pair_is_allowed`.
    struct NoResponsePairClient;
    #[async_trait]
    impl BulkImportPermissionCheck for NoResponsePairClient {
        async fn check_bulk_permissions(
            &self,
            request: authzed::v1::CheckBulkPermissionsRequest,
        ) -> Result<CheckBulkPermissionsResponse, tonic::Status> {
            let pairs = request
                .items
                .into_iter()
                .map(|_| CheckBulkPermissionsPair {
                    request: None,
                    response: None,
                })
                .collect();
            Ok(CheckBulkPermissionsResponse {
                checked_at: Some(ZedToken { token: "zed".into() }),
                pairs,
            })
        }

        async fn write_relationships(
            &self,
            _request: WriteRelationshipsRequest,
        ) -> Result<WriteRelationshipsResponse, tonic::Status> {
            unimplemented!()
        }
    }
    let gate = LiveAgentViewGate::new(
        std::sync::Arc::new(NoResponsePairClient),
        SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60)),
    );
    let session = SpiceDbSessionKey::new("alice", "acme");
    let principal = principal_with_subject("user:alice");
    let outcomes = gate.bulk_check_agent_view(&session, &principal, &[agent("a1")]).await;
    assert_eq!(outcomes, vec![AgentViewCheckOutcome::Denied]);
}

#[tokio::test]
async fn live_gate_pads_short_response_with_transport_error() {
    // SpiceDB returning fewer pairs than requested is a protocol
    // violation. Without the length-mismatch guard, `zip` would
    // truncate and the single-agent helper would mislabel
    // answered items as TransportError. The gate now pads the
    // missing tail with TransportError so the per-agent contract
    // holds and the protocol mismatch surfaces in audit.
    struct ShortResponseClient;
    #[async_trait]
    impl BulkImportPermissionCheck for ShortResponseClient {
        async fn check_bulk_permissions(
            &self,
            _request: authzed::v1::CheckBulkPermissionsRequest,
        ) -> Result<CheckBulkPermissionsResponse, tonic::Status> {
            // Ignore items, return a single pair regardless.
            Ok(CheckBulkPermissionsResponse {
                checked_at: Some(ZedToken { token: "zed".into() }),
                pairs: vec![CheckBulkPermissionsPair {
                    request: None,
                    response: Some(check_bulk_permissions_pair::Response::Item(
                        CheckBulkPermissionsResponseItem {
                            permissionship: Permissionship::HasPermission as i32,
                            partial_caveat_info: None,
                            debug_trace: None,
                        },
                    )),
                }],
            })
        }
        async fn write_relationships(
            &self,
            _request: WriteRelationshipsRequest,
        ) -> Result<WriteRelationshipsResponse, tonic::Status> {
            unimplemented!()
        }
    }
    let gate = LiveAgentViewGate::new(
        std::sync::Arc::new(ShortResponseClient),
        SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60)),
    );
    let session = SpiceDbSessionKey::new("alice", "acme");
    let principal = principal_with_subject("user:alice");
    let outcomes = gate
        .bulk_check_agent_view(&session, &principal, &[agent("a1"), agent("a2"), agent("a3")])
        .await;
    assert_eq!(
        outcomes,
        vec![
            AgentViewCheckOutcome::Allowed,
            AgentViewCheckOutcome::TransportError,
            AgentViewCheckOutcome::TransportError,
        ],
    );
}

#[tokio::test]
async fn live_gate_transport_error_returns_transport_error_per_agent() {
    // gRPC failure (timeout, connection refused, etc.) must
    // surface as TransportError -- distinct from a policy Denied
    // -- so audit consumers can tell 'we couldn't ask' from
    // 'policy said no'.
    struct ErrClient;
    #[async_trait]
    impl BulkImportPermissionCheck for ErrClient {
        async fn check_bulk_permissions(
            &self,
            _request: authzed::v1::CheckBulkPermissionsRequest,
        ) -> Result<CheckBulkPermissionsResponse, tonic::Status> {
            Err(tonic::Status::unavailable("backend unreachable"))
        }
        async fn write_relationships(
            &self,
            _request: WriteRelationshipsRequest,
        ) -> Result<WriteRelationshipsResponse, tonic::Status> {
            unimplemented!()
        }
    }
    let gate = LiveAgentViewGate::new(
        std::sync::Arc::new(ErrClient),
        SpiceDbSessionCache::new(ZedTokenTtl::from_secs(60)),
    );
    let session = SpiceDbSessionKey::new("alice", "acme");
    let principal = principal_with_subject("user:alice");
    let outcomes = gate
        .bulk_check_agent_view(&session, &principal, &[agent("a1"), agent("a2")])
        .await;
    assert_eq!(
        outcomes,
        vec![
            AgentViewCheckOutcome::TransportError,
            AgentViewCheckOutcome::TransportError
        ]
    );
}

#[test]
fn parse_subject_id_strips_slash_and_colon_prefixes() {
    assert_eq!(parse_subject_id("user/alice"), Some("alice".to_owned()));
    assert_eq!(parse_subject_id("user:alice"), Some("alice".to_owned()));
    assert_eq!(parse_subject_id("alice"), Some("alice".to_owned()));
    assert_eq!(parse_subject_id(""), None);
    assert_eq!(parse_subject_id("user/"), None);
    assert_eq!(parse_subject_id("user:"), None);
}
