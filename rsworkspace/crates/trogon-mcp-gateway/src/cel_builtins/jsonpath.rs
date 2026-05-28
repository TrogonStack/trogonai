use cel_interpreter::Value;

use super::errors::CelBuiltinsError;

pub(crate) const GET_NAME: &str = "jsonpath.get";
pub(crate) const SET_NAME: &str = "jsonpath.set";
pub(crate) const DELETE_NAME: &str = "jsonpath.delete";

pub fn get(doc: Value, expr: Value) -> Result<Value, CelBuiltinsError> {
    let _ = (doc, expr);
    Err(CelBuiltinsError::NotImplemented(GET_NAME))
}

pub fn set(doc: Value, expr: Value, value: Value) -> Result<Value, CelBuiltinsError> {
    let _ = (doc, expr, value);
    Err(CelBuiltinsError::NotImplemented(SET_NAME))
}

pub fn delete(doc: Value, expr: Value) -> Result<Value, CelBuiltinsError> {
    let _ = (doc, expr);
    Err(CelBuiltinsError::NotImplemented(DELETE_NAME))
}

#[cfg(test)]
mod tests {
    use cel_interpreter::Value;

    use super::{DELETE_NAME, GET_NAME, SET_NAME, delete, get, set};
    use crate::cel_builtins::errors::CelBuiltinsError;

    #[test]
    fn get_returns_not_implemented() {
        let err = get(Value::Null, Value::Null).unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(GET_NAME));
    }

    #[test]
    fn set_returns_not_implemented() {
        let err = set(Value::Null, Value::Null, Value::Null).unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(SET_NAME));
    }

    #[test]
    fn delete_returns_not_implemented() {
        let err = delete(Value::Null, Value::Null).unwrap_err();
        assert_eq!(err, CelBuiltinsError::NotImplemented(DELETE_NAME));
    }
}
