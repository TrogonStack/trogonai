//! SpiceDB-backed authorization checks from CEL policy.
//!
//! Fully consistent reads are required so policy decisions observe the latest tuple
//! state; snapshot reads would allow stale allows or denials during replication lag.

use cel_interpreter::Value;

use super::errors::CelBuiltinsError;

pub(crate) const BUILTIN_NAME: &str = "spicedb.check";

pub fn check(resource: Value, permission: Value, subject: Value) -> Result<Value, CelBuiltinsError> {
    let _ = (resource, permission, subject);
    Err(CelBuiltinsError::NotImplemented(BUILTIN_NAME))
}

#[cfg(test)]
mod tests {
    use cel_interpreter::Value;

    use super::{BUILTIN_NAME, check};
    use crate::cel_builtins::errors::CelBuiltinsError;

    #[test]
    fn check_returns_not_implemented() {
        let err = check(Value::Null, Value::Null, Value::Null).unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(BUILTIN_NAME));
    }
}
