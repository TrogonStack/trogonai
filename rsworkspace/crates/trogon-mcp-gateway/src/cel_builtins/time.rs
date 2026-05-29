//! Wall-clock access for time-bounded policy rules.

use cel_interpreter::Value;

use super::context::current_host_eval;
use super::errors::CelBuiltinsError;

pub(crate) const NOW_NAME: &str = "time.now";

pub fn now() -> Result<Value, CelBuiltinsError> {
    let host = current_host_eval().ok_or(CelBuiltinsError::policy_fault(
        NOW_NAME,
        "host eval context missing",
    ))?;
    Ok(Value::Int(host.now_unix_ms()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cel_interpreter::Value;

    use super::{NOW_NAME, now};
    use crate::cel_builtins::context::{with_host_eval, HostEvalContext};

    #[test]
    fn now_returns_pinned_clock_ms() {
        let host = HostEvalContext::for_tests().with_clock_ms(Arc::new(|| 1_700_000_000_123));
        let value = with_host_eval(&host, now).unwrap();
        assert_eq!(value, Value::Int(1_700_000_000_123));
    }

    #[test]
    fn now_requires_host_context() {
        let err = now().unwrap_err();
        assert!(err.to_string().contains(NOW_NAME));
    }
}
