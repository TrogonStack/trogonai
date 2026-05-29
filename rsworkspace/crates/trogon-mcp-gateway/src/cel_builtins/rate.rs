//! Windowed rate limiting from CEL policy (`local` scope in-process; `cluster` KV TBD).

use cel_interpreter::Value;

use super::context::current_host_eval;
use super::errors::CelBuiltinsError;
use super::value::{duration_from_value, expect_string, expect_u32};

pub(crate) const ACQUIRE_NAME: &str = "rate.acquire";

pub fn acquire(
    scope: Value,
    key: Value,
    budget: Value,
    window: Value,
) -> Result<Value, CelBuiltinsError> {
    let host = current_host_eval().ok_or(CelBuiltinsError::policy_fault(
        ACQUIRE_NAME,
        "host eval context missing",
    ))?;
    let scope = expect_string(scope, ACQUIRE_NAME, 0)?;
    let key = expect_string(key, ACQUIRE_NAME, 1)?;
    let budget = expect_u32(budget, ACQUIRE_NAME, 2)?;
    let window = duration_from_value(window, ACQUIRE_NAME, 3)?;
    Ok(Value::Bool(
        host.rate_acquire(scope.as_str(), key.as_str(), budget, window)?,
    ))
}

#[cfg(test)]
mod tests {
    use cel_interpreter::Value;

    use super::{ACQUIRE_NAME, acquire};
    use crate::cel_builtins::context::{with_host_eval, HostEvalContext};
    use crate::cel_builtins::errors::HostFailure;

    fn s(v: &str) -> Value {
        Value::from(v.to_string())
    }

    #[test]
    fn local_acquire_consumes_budget() {
        let host = HostEvalContext::for_tests();
        with_host_eval(&host, || {
            assert_eq!(
                acquire(
                    s("local"),
                    s("tool/deploy"),
                    Value::Int(1),
                    Value::Int(60),
                )
                .unwrap(),
                Value::Bool(true)
            );
            assert_eq!(
                acquire(
                    s("local"),
                    s("tool/deploy"),
                    Value::Int(1),
                    Value::Int(60),
                )
                .unwrap(),
                Value::Bool(false)
            );
        });
    }

    #[test]
    fn cluster_scope_is_transient_failure() {
        let host = HostEvalContext::for_tests();
        let err = with_host_eval(&host, || {
            acquire(
                s("cluster"),
                s("k"),
                Value::Int(10),
                Value::Int(60),
            )
        })
        .unwrap_err();
        assert_eq!(err.host_failure(), Some(HostFailure::Transient));
        assert!(err.to_string().contains(ACQUIRE_NAME));
    }

    #[test]
    fn zero_budget_is_policy_fault() {
        let host = HostEvalContext::for_tests();
        let err = with_host_eval(&host, || {
            acquire(
                s("local"),
                s("k"),
                Value::Int(0),
                Value::Int(60),
            )
        })
        .unwrap_err();
        assert_eq!(err.host_failure(), Some(HostFailure::Permanent));
    }
}
