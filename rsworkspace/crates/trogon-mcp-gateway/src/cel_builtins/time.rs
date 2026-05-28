//! Wall-clock access for time-bounded policy rules.
//!
//! Deterministic replay requires pinning or mocking `time.now`; live values make
//! historical audit re-evaluation disagree with the original decision.

use cel_interpreter::Value;

use super::errors::CelBuiltinsError;

pub(crate) const NOW_NAME: &str = "time.now";

pub fn now() -> Result<Value, CelBuiltinsError> {
    Err(CelBuiltinsError::NotImplemented(NOW_NAME))
}

#[cfg(test)]
mod tests {
    use super::{NOW_NAME, now};
    use crate::cel_builtins::errors::CelBuiltinsError;

    #[test]
    fn now_returns_not_implemented() {
        let err = now().unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(NOW_NAME));
    }
}
