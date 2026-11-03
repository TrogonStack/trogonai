use a2a_pack::Tier1Permission;
use trogon_std::env::InMemoryEnv;

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

fn caller(kind: &str, id: &str) -> SubjectPrincipal {
    SubjectPrincipal::from_tokens(kind, id).expect("principal")
}

#[test]
fn tier1_principal_from_caller_carries_only_the_derived_subject() {
    let principal = tier1_principal_from_caller(&CallerSpiceDbSubject::from(&caller("person", "alice")));

    assert_eq!(principal, SpiceDbPrincipal::new("user/alice"));
}

#[test]
fn tier1_session_from_caller_keys_on_the_subject_principal() {
    let session = tier1_session_from_caller(&caller("person", "alice"), "acme");

    assert_eq!(session.sub(), "person.alice");
    assert_eq!(session.account(), "acme");
    assert_ne!(session, tier1_session_from_caller(&caller("agent", "alice"), "acme"));
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

struct AllNotUnicodeEnv;
impl ReadEnv for AllNotUnicodeEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotUnicode(std::ffi::OsString::from("garbage")))
    }
}

#[test]
fn tier1_enabled_handles_non_unicode_env_as_disabled() {
    let env = AllNotUnicodeEnv;
    assert!(!tier1_enabled(&env));
}

#[test]
fn gateway_tier1_layer_debug_emits_summary_fields() {
    let layer = GatewayTier1Layer::noop();
    let debug = format!("{layer:?}");
    assert!(debug.contains("gate_is_enabled"));
    assert!(debug.contains("has_owner_emitter"));
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

#[cfg(feature = "spicedb")]
#[test]
fn gateway_tier1_layer_noop_has_disabled_discovery_view() {
    let layer = GatewayTier1Layer::noop();
    assert!(!layer.discovery_view.is_enabled());
    assert!(format!("{layer:?}").contains("discovery_view_is_enabled"));
}

#[test]
fn gateway_tier1_layer_noop_has_disabled_gates() {
    let layer = GatewayTier1Layer::noop();
    assert!(!layer.gate.is_enabled());
    assert!(layer.owner_emitter.is_none());
}

#[test]
fn owner_emit_error_carries_its_message() {
    let error = OwnerEmitError::new("backend unavailable");
    assert_eq!(error.to_string(), "owner tuple emit failed: backend unavailable");
}

#[cfg(not(feature = "spicedb"))]
mod default_build {
    use super::*;

    #[tokio::test]
    async fn from_env_returns_noop_when_disabled() {
        let env = InMemoryEnv::new();
        let layer = Tier1SpiceDbConfig::from_env(&env).await.expect("noop ok");
        assert!(!layer.gate.is_enabled());
        assert!(layer.owner_emitter.is_none());
    }

    #[tokio::test]
    async fn from_env_refuses_to_run_unenforced_when_enabled() {
        for raw in ["1", "true", " on ", "\tyes\n"] {
            let env = InMemoryEnv::new();
            env.set(ENV_TIER1_SPICEDB_ENABLED, raw);
            let error = Tier1SpiceDbConfig::from_env(&env)
                .await
                .expect_err("enabled gate without the feature must not boot");
            assert_eq!(
                error,
                Tier1SpiceDbBuildError::NotCompiledIn {
                    enabled_var: ENV_TIER1_SPICEDB_ENABLED
                },
                "{raw:?}",
            );
        }
    }

    #[tokio::test]
    async fn from_env_stays_noop_when_explicitly_off() {
        let env = InMemoryEnv::new();
        env.set(ENV_TIER1_SPICEDB_ENABLED, "off");
        assert!(Tier1SpiceDbConfig::from_env(&env).await.is_ok());
    }
}
