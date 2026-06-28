use std::ffi::OsString;

use super::super::{EnumerateEnv, ReadEnv};
use super::*;

#[test]
fn test_in_memory_env_set_and_get() {
    let env = InMemoryEnv::new();
    env.set("TEST_VAR", "test_value");

    assert_eq!(env.var("TEST_VAR").unwrap(), "test_value");
}

#[test]
fn test_in_memory_env_not_present() {
    let env = InMemoryEnv::new();

    assert!(matches!(env.var("NONEXISTENT"), Err(std::env::VarError::NotPresent)));
}

#[test]
fn test_in_memory_env_remove() {
    let env = InMemoryEnv::new();
    env.set("TEST_VAR", "test_value");
    env.remove("TEST_VAR");

    assert!(matches!(env.var("TEST_VAR"), Err(std::env::VarError::NotPresent)));
}

#[test]
fn test_in_memory_env_contains() {
    let env = InMemoryEnv::new();
    env.set("TEST_VAR", "test_value");

    assert!(env.contains("TEST_VAR"));
    assert!(!env.contains("NONEXISTENT"));
}

#[test]
fn test_in_memory_env_clear() {
    let env = InMemoryEnv::new();
    env.set("TEST_VAR_1", "value1");
    env.set("TEST_VAR_2", "value2");
    env.clear();

    assert!(!env.contains("TEST_VAR_1"));
    assert!(!env.contains("TEST_VAR_2"));
}

#[test]
fn test_in_memory_env_default() {
    let env = InMemoryEnv::default();
    env.set("KEY", "val");
    assert_eq!(env.var("KEY").unwrap(), "val");
}

#[test]
fn test_in_memory_env_overwrite() {
    let env = InMemoryEnv::new();
    env.set("KEY", "v1");
    assert_eq!(env.var("KEY").unwrap(), "v1");

    env.set("KEY", "v2");
    assert_eq!(env.var("KEY").unwrap(), "v2");
}

#[test]
fn test_in_memory_env_var_os() {
    let env = InMemoryEnv::new();
    env.set("TEST_VAR", "test_value");

    assert_eq!(env.var_os("TEST_VAR"), Some(OsString::from("test_value")));
    assert_eq!(env.var_os("NONEXISTENT"), None);
}

#[test]
fn test_in_memory_env_vars() {
    let env = InMemoryEnv::new();
    env.set("A", "1");
    env.set("B", "2");

    let mut vars = env.vars();
    vars.sort();
    assert_eq!(vars, vec![("A".to_string(), "1".to_string()), ("B".to_string(), "2".to_string())]);
}

#[test]
fn test_in_memory_env_vars_os() {
    let env = InMemoryEnv::new();
    env.set("A", "1");

    assert_eq!(env.vars_os(), vec![(OsString::from("A"), OsString::from("1"))]);
}

#[test]
fn test_generic_function_with_in_memory_env() {
    fn get_value_or_default<E: ReadEnv>(env: &E, key: &str, default: &str) -> String {
        env.var(key).unwrap_or_else(|_| default.to_string())
    }

    let mem_env = InMemoryEnv::new();
    mem_env.set("MY_VAR", "custom_value");
    assert_eq!(get_value_or_default(&mem_env, "MY_VAR", "default"), "custom_value");
    assert_eq!(get_value_or_default(&mem_env, "MISSING", "default"), "default");
}
