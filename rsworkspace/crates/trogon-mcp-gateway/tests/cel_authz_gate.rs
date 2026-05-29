//! Integration tests for the Phase 1 CEL SpiceDB authorization gate.
//!
//! The gate expression `mcp.method == "tools/call" || mcp.method == "resources/read"`
//! selects when the SpiceDB-backed [`trogon_mcp_gateway::authz::PermissionChecker`] hook
//! runs on ingress JSON-RPC requests. Methods outside that set must bypass SpiceDB entirely.
//!
//! Cross-refs:
//! - `MCP_GATEWAY_PLAN.md` Block D Phase 1 (CEL gate + SpiceDB hook)
//! - `docs/adr/0008-policy-dsl.md` (CEL policy DSL)
//! - `docs/identity/policy-dsl-choice.md` (DSL selection rationale)
//!
//! NATS harness cases remain `#[ignore]` until a counting checker spy is exposed on the
//! gateway ingress path; unit cases below exercise gate selection and host-builtin wiring.

use std::sync::Arc;

use cel_interpreter::{to_value, Context, Program, Value};
use trogon_mcp_gateway::cel_builtins::{register_all, with_host_eval, HostEvalContext};
use trogon_mcp_gateway::policy::SpicedbGatePolicy;

mod gate_selection {
    use super::*;

    #[test]
    fn gate_is_true_for_tools_call() {
        let policy = SpicedbGatePolicy::phase1_hardcoded().unwrap();
        assert!(policy.requires_spicedb_for_method("tools/call").unwrap());
    }

    #[test]
    fn gate_is_true_for_resources_read() {
        let policy = SpicedbGatePolicy::phase1_hardcoded().unwrap();
        assert!(policy.requires_spicedb_for_method("resources/read").unwrap());
    }

    #[test]
    fn gate_is_false_for_tools_list() {
        let policy = SpicedbGatePolicy::phase1_hardcoded().unwrap();
        assert!(!policy.requires_spicedb_for_method("tools/list").unwrap());
    }

    #[test]
    fn gate_is_false_for_initialize() {
        let policy = SpicedbGatePolicy::phase1_hardcoded().unwrap();
        assert!(!policy.requires_spicedb_for_method("initialize").unwrap());
    }
}

mod host_builtins_wiring {
    use super::*;

    fn eval(source: &str) -> Value {
        let mut ctx = Context::default();
        register_all(&mut ctx).unwrap();
        let mcp = to_value(serde_json::json!({ "method": "tools/call" })).unwrap();
        ctx.add_variable_from_value("mcp", mcp);
        let host = HostEvalContext::for_tests().with_clock_ms(Arc::new(|| 1_700_000_000_000));
        let program = Program::compile(source).unwrap();
        with_host_eval(&host, || program.execute(&ctx)).unwrap()
    }

    #[test]
    fn cache_roundtrip_in_policy_context() {
        assert_eq!(
            eval(
                r#"cache.set("k", {"v": 1}, duration("30s")) && cache.get("k").v == 1"#
            ),
            Value::Bool(true)
        );
    }

    #[test]
    fn jsonpath_has_on_mcp_params() {
        assert_eq!(
            eval(
                r#"jsonpath.has(mcp, "$.method") && jsonpath.extract(mcp, "$.method") == "tools/call""#
            ),
            Value::Bool(true)
        );
    }

    #[test]
    fn audit_emit_and_time_now_succeed() {
        assert_eq!(
            eval(
                r#"audit.emit({"rule": "gate"}) && time.now() == 1700000000000"#
            ),
            Value::Bool(true)
        );
    }

    #[test]
    fn rate_acquire_local_budget() {
        assert_eq!(
            eval(
                r#"rate.acquire("local", "gate", 1, duration("1m")) && !rate.acquire("local", "gate", 1, duration("1m"))"#
            ),
            Value::Bool(true)
        );
    }
}

mod gate_selects_spicedb {
    #[tokio::test]
    #[ignore = "needs NATS harness and counting PermissionChecker spy on gateway ingress"]
    async fn gate_evaluates_true_for_tools_call_invokes_spicedb_checker() {
        unimplemented!("populate when gateway exposes checker invocation counter");
    }

    #[tokio::test]
    #[ignore = "needs NATS harness and counting PermissionChecker spy on gateway ingress"]
    async fn gate_evaluates_true_for_resources_read_invokes_spicedb_checker() {
        unimplemented!("populate when gateway exposes checker invocation counter");
    }
}

mod gate_bypasses_spicedb {
    #[tokio::test]
    #[ignore = "needs NATS harness and counting PermissionChecker spy on gateway ingress"]
    async fn gate_evaluates_false_for_tools_list_bypasses_spicedb_checker() {
        unimplemented!("populate when gateway exposes checker invocation counter");
    }

    #[tokio::test]
    #[ignore = "needs NATS harness and counting PermissionChecker spy on gateway ingress"]
    async fn gate_evaluates_false_for_initialize_bypasses_spicedb_checker() {
        unimplemented!("populate when gateway exposes checker invocation counter");
    }
}
