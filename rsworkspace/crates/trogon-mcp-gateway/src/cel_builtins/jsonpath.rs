//! JSONPath read helpers for request envelope inspection (RFC 9535 subset).

use std::sync::Arc;

use cel_interpreter::Value;

use super::context::current_host_eval;
use super::errors::CelBuiltinsError;
use super::value::{expect_string, json_to_value, value_to_json};

pub(crate) const QUERY_NAME: &str = "jsonpath.query";
pub(crate) const EXTRACT_NAME: &str = "jsonpath.extract";
pub(crate) const GET_NAME: &str = "jsonpath.get";
pub(crate) const HAS_NAME: &str = "jsonpath.has";
pub(crate) const SET_NAME: &str = "jsonpath.set";
pub(crate) const DELETE_NAME: &str = "jsonpath.delete";

pub fn query(doc: Value, path: Value) -> Result<Value, CelBuiltinsError> {
    let _host = current_host_eval();
    let doc = value_to_json(&doc)?;
    let path = expect_string(path, QUERY_NAME, 1)?;
    let matches = eval_jsonpath(&doc, path.as_str())?;
    let values = matches
        .into_iter()
        .map(json_to_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::List(Arc::new(values)))
}

pub fn extract(doc: Value, path: Value) -> Result<Value, CelBuiltinsError> {
    let matches = query_matches(doc, path, EXTRACT_NAME)?;
    match matches.len() {
        0 => Err(CelBuiltinsError::policy_fault(EXTRACT_NAME, "jsonpath no match")),
        1 => json_to_value(matches.into_iter().next().expect("one match")),
        _ => Err(CelBuiltinsError::policy_fault(
            EXTRACT_NAME,
            "jsonpath_ambiguous",
        )),
    }
}

pub fn get(doc: Value, path: Value) -> Result<Value, CelBuiltinsError> {
    extract(doc, path)
}

pub fn has(doc: Value, path: Value) -> Result<Value, CelBuiltinsError> {
    let matches = query_matches(doc, path, HAS_NAME)?;
    Ok(Value::Bool(!matches.is_empty()))
}

pub fn set(_doc: Value, _path: Value, _value: Value) -> Result<Value, CelBuiltinsError> {
    Err(CelBuiltinsError::NotImplemented(SET_NAME))
}

pub fn delete(_doc: Value, _path: Value) -> Result<Value, CelBuiltinsError> {
    Err(CelBuiltinsError::NotImplemented(DELETE_NAME))
}

fn query_matches(
    doc: Value,
    path: Value,
    name: &'static str,
) -> Result<Vec<serde_json::Value>, CelBuiltinsError> {
    let doc = value_to_json(&doc)?;
    let path = expect_string(path, name, 1)?;
    eval_jsonpath(&doc, path.as_str())
}

fn eval_jsonpath(doc: &serde_json::Value, path: &str) -> Result<Vec<serde_json::Value>, CelBuiltinsError> {
    if !path.starts_with('$') {
        return Err(CelBuiltinsError::policy_fault(
            QUERY_NAME,
            "jsonpath must start with $",
        ));
    }
    if path == "$" {
        return Ok(vec![doc.clone()]);
    }
    let pointer = to_json_pointer(path).ok_or_else(|| {
        CelBuiltinsError::policy_fault(QUERY_NAME, "invalid jsonpath")
    })?;
    Ok(value_at_pointer(doc, &pointer)
        .into_iter()
        .cloned()
        .collect())
}

fn tokenize_json_path(path: &str) -> Option<Vec<String>> {
    let stripped = path.strip_prefix('$')?;
    if stripped.is_empty() {
        return Some(vec![]);
    }
    let stripped = stripped.strip_prefix('.').unwrap_or(stripped);
    if stripped.is_empty() {
        return Some(vec![]);
    }
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = stripped.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '[' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                let mut index = String::new();
                for inner in chars.by_ref() {
                    if inner == ']' {
                        break;
                    }
                    index.push(inner);
                }
                let index = index.trim().trim_matches(['\'', '"']).to_string();
                if !index.is_empty() {
                    tokens.push(index);
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Some(tokens)
}

fn to_json_pointer(path: &str) -> Option<String> {
    let tokens = tokenize_json_path(path)?;
    if tokens.is_empty() {
        return Some(String::new());
    }
    let mut pointer = String::from("/");
    for segment in tokens {
        pointer.push_str(&segment.replace('~', "~0").replace('/', "~1"));
        pointer.push('/');
    }
    Some(pointer.trim_end_matches('/').to_string())
}

fn value_at_pointer<'a>(root: &'a serde_json::Value, pointer: &str) -> Vec<&'a serde_json::Value> {
    if pointer.is_empty() {
        return vec![root];
    }
    root.pointer(pointer).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cel_interpreter::Value;

    use super::{extract, get, has, query, EXTRACT_NAME, QUERY_NAME};
    use crate::cel_builtins::context::{with_host_eval, HostEvalContext};
    use crate::cel_builtins::errors::HostFailure;
    use crate::cel_builtins::value::json_to_value;

    fn s(v: &str) -> Value {
        Value::from(v.to_string())
    }

    fn doc() -> Value {
        json_to_value(serde_json::json!({
            "foo": {"bar": 42},
            "items": [{"id": "a"}, {"id": "b"}]
        }))
        .unwrap()
    }

    #[test]
    fn query_returns_matches() {
        let host = HostEvalContext::for_tests();
        let out = with_host_eval(&host, || query(doc(), s("$.foo.bar"))).unwrap();
        match out {
            Value::List(items) => assert_eq!(items.len(), 1),
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn query_zero_matches_returns_empty_list() {
        let host = HostEvalContext::for_tests();
        let out =
            with_host_eval(&host, || query(doc(), s("$.missing"))).unwrap();
        assert_eq!(out, Value::List(Arc::new(vec![])));
    }

    #[test]
    fn extract_single_match_ok() {
        let host = HostEvalContext::for_tests();
        let out = with_host_eval(&host, || extract(doc(), s("$.foo.bar"))).unwrap();
        assert_eq!(out, Value::Int(42));
    }

    #[test]
    fn extract_zero_matches_is_policy_fault() {
        let host = HostEvalContext::for_tests();
        let err = with_host_eval(&host, || extract(doc(), s("$.missing"))).unwrap_err();
        assert_eq!(err.host_failure(), Some(HostFailure::Permanent));
        assert!(err.to_string().contains(EXTRACT_NAME));
    }

    #[test]
    fn get_aliases_extract() {
        let host = HostEvalContext::for_tests();
        let out = with_host_eval(&host, || get(doc(), s("$.foo.bar"))).unwrap();
        assert_eq!(out, Value::Int(42));
    }

    #[test]
    fn has_reflects_presence() {
        let host = HostEvalContext::for_tests();
        let yes = with_host_eval(&host, || has(doc(), s("$.foo.bar"))).unwrap();
        let no = with_host_eval(&host, || has(doc(), s("$.missing"))).unwrap();
        assert_eq!(yes, Value::Bool(true));
        assert_eq!(no, Value::Bool(false));
    }

    #[test]
    fn invalid_path_is_policy_fault() {
        let host = HostEvalContext::for_tests();
        let err = with_host_eval(&host, || query(doc(), s("not-a-path"))).unwrap_err();
        assert_eq!(err.host_failure(), Some(HostFailure::Permanent));
        assert!(err.to_string().contains(QUERY_NAME));
    }

    #[test]
    fn bracket_index_path_works() {
        let host = HostEvalContext::for_tests();
        let out = with_host_eval(
            &host,
            || get(doc(), s("$.items[0].id")),
        )
        .unwrap();
        assert_eq!(out, s("a"));
    }
}
