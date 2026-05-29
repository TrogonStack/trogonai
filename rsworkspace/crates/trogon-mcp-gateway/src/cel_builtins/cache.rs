//! Per-evaluation memoisation for expensive builtin calls.

use cel_interpreter::Value;

use super::context::current_host_eval;
use super::errors::CelBuiltinsError;
use super::value::{duration_secs, expect_string, json_to_value, value_to_json};

pub(crate) const GET_NAME: &str = "cache.get";
pub(crate) const SET_NAME: &str = "cache.set";

pub fn get(key: Value) -> Result<Value, CelBuiltinsError> {
    let host = current_host_eval().ok_or(CelBuiltinsError::policy_fault(
        GET_NAME,
        "host eval context missing",
    ))?;
    let key = expect_string(key, GET_NAME, 0)?;
    match host.cache_get(key.as_str())? {
        Some(json) => json_to_value(json),
        None => Ok(Value::Null),
    }
}

pub fn set(key: Value, value: Value, ttl: Value) -> Result<Value, CelBuiltinsError> {
    let host = current_host_eval().ok_or(CelBuiltinsError::policy_fault(
        SET_NAME,
        "host eval context missing",
    ))?;
    let key = expect_string(key, SET_NAME, 0)?;
    let ttl_secs = duration_secs(ttl, SET_NAME, 2)?;
    let json = value_to_json(&value)?;
    Ok(Value::Bool(
        host.cache_set(key.as_str(), json, ttl_secs)?,
    ))
}

#[cfg(test)]
mod tests {
    use cel_interpreter::Value;

    use super::{GET_NAME, get, set};
    use crate::cel_builtins::context::{with_host_eval, HostEvalContext};
    use crate::cel_builtins::errors::HostFailure;

    fn s(v: &str) -> Value {
        Value::from(v.to_string())
    }

    #[test]
    fn get_miss_returns_null() {
        let host = HostEvalContext::for_tests();
        let result = with_host_eval(&host, || get(s("missing"))).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn set_then_get_roundtrip() {
        let host = HostEvalContext::for_tests();
        with_host_eval(&host, || {
            set(s("zed"), s("tok"), Value::Int(60)).unwrap();
            let got = get(s("zed")).unwrap();
            assert_eq!(got, s("tok"));
        });
    }

    #[test]
    fn oversize_value_rejected_without_error() {
        let host = HostEvalContext::for_tests();
        let huge = "x".repeat(70_000);
        let accepted = with_host_eval(&host, || {
            set(s("big"), s(&huge), Value::Int(60))
        })
        .unwrap();
        assert_eq!(accepted, Value::Bool(false));
    }

    #[test]
    fn key_too_long_is_policy_fault() {
        let host = HostEvalContext::for_tests();
        let key = "k".repeat(300);
        let err = with_host_eval(&host, || get(s(&key))).unwrap_err();
        assert_eq!(err.host_failure(), Some(HostFailure::Permanent));
        assert!(err.to_string().contains(GET_NAME));
    }

    #[test]
    fn ttl_clamped_to_bounds() {
        let host = HostEvalContext::for_tests();
        with_host_eval(&host, || {
            assert_eq!(
                set(s("a"), s("v"), Value::Int(0)).unwrap(),
                Value::Bool(true)
            );
            assert!(get(s("a")).unwrap() != Value::Null);
        });
    }
}
