//! Token-bucket rate limiting from CEL policy.
//!
//! Backing store choice (in-memory vs shared) is TBD and affects fairness across
//! gateway replicas.

use cel_interpreter::Value;

use super::errors::CelBuiltinsError;

pub(crate) const ACQUIRE_NAME: &str = "rate.acquire";

pub fn acquire(key: Value, capacity: Value, refill_per_sec: Value) -> Result<Value, CelBuiltinsError> {
    let _ = (key, capacity, refill_per_sec);
    Err(CelBuiltinsError::NotImplemented(ACQUIRE_NAME))
}

#[cfg(test)]
mod tests {
    use cel_interpreter::Value;

    use super::{ACQUIRE_NAME, acquire};
    use crate::cel_builtins::errors::CelBuiltinsError;

    #[test]
    fn acquire_returns_not_implemented() {
        let err = acquire(Value::Null, Value::Null, Value::Null).unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(ACQUIRE_NAME));
    }
}
