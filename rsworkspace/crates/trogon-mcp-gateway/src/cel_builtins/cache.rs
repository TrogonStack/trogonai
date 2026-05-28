//! Per-evaluation memoisation for expensive builtin calls.
//!
//! Cross-evaluation caching would let later replays observe different memo hits and
//! break audit determinism for the same policy inputs.

use cel_interpreter::Value;

use super::errors::CelBuiltinsError;

pub(crate) const GET_NAME: &str = "cache.get";
pub(crate) const SET_NAME: &str = "cache.set";

pub fn get(key: Value) -> Result<Value, CelBuiltinsError> {
    let _ = key;
    Err(CelBuiltinsError::NotImplemented(GET_NAME))
}

pub fn set(key: Value, value: Value, ttl_secs: Value) -> Result<Value, CelBuiltinsError> {
    let _ = (key, value, ttl_secs);
    Err(CelBuiltinsError::NotImplemented(SET_NAME))
}

#[cfg(test)]
mod tests {
    use cel_interpreter::Value;

    use super::{GET_NAME, SET_NAME, get, set};
    use crate::cel_builtins::errors::CelBuiltinsError;

    #[test]
    fn get_returns_not_implemented() {
        let err = get(Value::Null).unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(GET_NAME));
    }

    #[test]
    fn set_returns_not_implemented() {
        let err = set(Value::Null, Value::Null, Value::Null).unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(SET_NAME));
    }
}
