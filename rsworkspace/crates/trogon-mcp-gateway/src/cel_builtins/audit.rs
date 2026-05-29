//! Audit emission from CEL decision policies.

use cel_interpreter::Value;

use super::context::current_host_eval;
use super::errors::CelBuiltinsError;
use super::value::{expect_map, map_to_json_object};

pub(crate) const EMIT_NAME: &str = "audit.emit";

pub const RESERVED_AUDIT_KEYS: &[&str] = &[
    "schema",
    "ts",
    "trace_id",
    "span_id",
    "instance_id",
    "tenant",
    "session_id",
    "caller",
    "subject_in",
    "subject_out",
    "direction",
    "method",
    "method_root",
    "tool",
    "decision",
    "rules_fired",
    "rewrites",
    "spicedb",
    "error",
    "latency_us",
    "agent_id",
    "agent_version",
    "wkl",
    "purpose",
    "act_chain",
];

pub fn emit(extra_fields: Value) -> Result<Value, CelBuiltinsError> {
    let host = current_host_eval().ok_or(CelBuiltinsError::policy_fault(
        EMIT_NAME,
        "host eval context missing",
    ))?;
    let map = expect_map(extra_fields, EMIT_NAME, 0)?;
    let fields = map_to_json_object(map)?;
    Ok(Value::Bool(host.audit_emit(fields)?))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cel_interpreter::objects::Key;
    use cel_interpreter::Value;

    use super::{EMIT_NAME, emit};
    use crate::cel_builtins::context::{with_host_eval, HostEvalContext};
    use crate::cel_builtins::errors::HostFailure;

    #[test]
    fn emit_merges_fields_into_journal() {
        let host = HostEvalContext::for_tests();
        with_host_eval(&host, || {
            let mut map = HashMap::new();
            map.insert(
                Key::from("policy_rule"),
                Value::from("business_hours".to_string()),
            );
            assert_eq!(emit(Value::Map(map.into())).unwrap(), Value::Bool(true));
        });
        let journal = host.audit.lock().expect("audit lock");
        assert_eq!(
            journal.extra.get("policy_rule").and_then(|v| v.as_str()),
            Some("business_hours")
        );
    }

    #[test]
    fn reserved_top_level_key_is_policy_fault() {
        let host = HostEvalContext::for_tests();
        let err = with_host_eval(&host, || {
            let mut map = HashMap::new();
            map.insert(Key::from("trace_id"), Value::from("x".to_string()));
            emit(Value::Map(map.into()))
        })
        .unwrap_err();
        assert_eq!(err.host_failure(), Some(HostFailure::Permanent));
        assert!(err.to_string().contains(EMIT_NAME));
    }
}
