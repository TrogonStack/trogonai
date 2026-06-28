use a2a_pack::Tier1Permission;
use trogon_std::env::{InMemoryEnv, ReadEnv};

use super::*;

fn agent(s: &str) -> A2aAgentId {
    A2aAgentId::new(s).expect("nats-safe test agent")
}

fn empty_principal() -> SpiceDbPrincipal {
    SpiceDbPrincipal(serde_json::json!({}))
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
    assert_eq!(owner.subject_type, "user");
    assert_eq!(owner.subject_id, "alice");
}

#[test]
fn derive_tuple_handles_each_a2a_method() {
    // Covers the round-trip from typed A2aMethod through the
    // a2a-pack resource-tuple table. A regression in either side
    // (method-slug spelling, table row, derive logic) trips this.
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

#[test]
fn from_env_returns_noop_when_disabled() {
    let env = InMemoryEnv::new();
    let layer = Tier1SpiceDbConfig::from_env(&env).expect("noop ok");
    assert!(!layer.gate.is_enabled());
    assert!(layer.owner_emitter.is_none());
    assert!(!layer.discovery_view.is_enabled());
}

#[test]
fn from_env_errors_when_enabled_without_credentials() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "true");
    let err = Tier1SpiceDbConfig::from_env(&env).expect_err("must require credentials");
    assert!(
        matches!(err, Tier1SpiceDbBuildError::PartialConfig(_)),
        "missing endpoint+token must surface as PartialConfig, got {err:?}",
    );
}

#[test]
fn from_env_errors_when_endpoint_set_without_token() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "true");
    env.set(ENV_TIER1_SPICEDB_ENDPOINT, "https://spicedb.example/");
    let err = Tier1SpiceDbConfig::from_env(&env).expect_err("must require token");
    assert!(matches!(err, Tier1SpiceDbBuildError::PartialConfig(_)));
}

#[test]
fn from_env_errors_when_token_set_without_endpoint() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "true");
    env.set(ENV_TIER1_SPICEDB_TOKEN, "tok");
    let err = Tier1SpiceDbConfig::from_env(&env).expect_err("must require endpoint");
    assert!(matches!(err, Tier1SpiceDbBuildError::PartialConfig(_)));
}

#[test]
fn from_env_errors_when_fully_configured_until_live_gate_ships() {
    // Tier-1 enabled + credentials present hits the deliberate
    // "live gate ships later" stub. The intent: an operator with
    // credentials configured doesn't silently get the Noop gate
    // (which would shadow-allow everything). Once the live-gate
    // slice lands, this assertion flips to checking
    // `layer.gate.is_enabled()`.
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_SPICEDB_ENABLED, "true");
    env.set(ENV_TIER1_SPICEDB_ENDPOINT, "https://spicedb.example/");
    env.set(ENV_TIER1_SPICEDB_TOKEN, "tok");
    let err = Tier1SpiceDbConfig::from_env(&env).expect_err("live gate not yet wired");
    assert!(matches!(err, Tier1SpiceDbBuildError::Connect(_)));
}

#[test]
fn from_env_rejects_invalid_ttl() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER1_ZEDTOKEN_TTL_SECS, "not-a-number");
    let err = Tier1SpiceDbConfig::from_env(&env).expect_err("must reject");
    assert!(matches!(err, Tier1SpiceDbBuildError::InvalidZedTokenTtl(_)));
}

#[test]
fn from_env_default_ttl_when_unset() {
    let env = InMemoryEnv::new();
    let layer = Tier1SpiceDbConfig::from_env(&env).expect("ok");
    let _ = layer; // smoke
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

/// Env fixture that returns `NotUnicode` for every key. Surfaces
/// the byte-stream env paths `InMemoryEnv` can't model so the
/// per-key `NotUnicode` branches in `tier1_zed_token_ttl` and
/// `optional_tier1_credentials` are reachable from tests.
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

/// Env that returns the supplied value for `key_ok` and
/// `NotUnicode` for the other key. Lets each side of the
/// `optional_tier1_credentials` branch surface independently.
struct PartiallyNotUnicodeEnv {
    /// (key whose lookup succeeds, value to return).
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
    // Tier1SpiceDbConfig::from_env -> GatewayTier1Layer Debug is
    // surfaced in audit/telemetry; pin the field names so a
    // renamed field doesn't silently drop from the log shape.
    let layer = GatewayTier1Layer::noop();
    let debug = format!("{layer:?}");
    assert!(
        debug.contains("gate_is_enabled"),
        "debug must carry gate_is_enabled: {debug}"
    );
    assert!(debug.contains("has_owner_emitter"));
    assert!(debug.contains("discovery_view_is_enabled"));
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

#[test]
fn gateway_tier1_layer_noop_has_disabled_gates() {
    let layer = GatewayTier1Layer::noop();
    assert!(!layer.gate.is_enabled());
    assert!(layer.owner_emitter.is_none());
    assert!(!layer.discovery_view.is_enabled());
}

#[test]
fn session_cache_alias_round_trips() {
    let _ = SpiceDbTier1SessionCache::new(a2a_nats::catalog::import_gate::ZedTokenTtl::from_secs(60));
}
