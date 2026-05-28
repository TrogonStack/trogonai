//! Audit emission from CEL decision policies.
//!
//! Side-effecting builtins belong only in decision policies: predicate policies must
//! stay pure so the same inputs always yield the same boolean.

use cel_interpreter::Value;

use super::errors::CelBuiltinsError;

pub(crate) const EMIT_NAME: &str = "audit.emit";

pub fn emit(category: Value, fields: Value) -> Result<Value, CelBuiltinsError> {
    let _ = (category, fields);
    Err(CelBuiltinsError::NotImplemented(EMIT_NAME))
}

#[cfg(test)]
mod tests {
    use cel_interpreter::Value;

    use super::{EMIT_NAME, emit};
    use crate::cel_builtins::errors::CelBuiltinsError;

    #[test]
    fn emit_returns_not_implemented() {
        let err = emit(Value::Null, Value::Null).unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(EMIT_NAME));
    }
}
