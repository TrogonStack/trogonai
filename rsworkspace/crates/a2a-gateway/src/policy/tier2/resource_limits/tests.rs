use std::collections::BTreeMap;

use trogon_std::env::InMemoryEnv;

use super::*;

#[test]
fn default_limits_match_agt_adapted_values() {
    let limits = Tier2ResourceLimits::default();
    assert_eq!(limits.max_headers_bytes(), 1_048_576);
    assert_eq!(limits.max_params_bytes(), 1_048_576);
    assert_eq!(limits.max_nesting_depth(), 64);
}

#[test]
fn new_round_trips_explicit_values() {
    let limits = Tier2ResourceLimits::new(10, 20, 3);
    assert_eq!(limits.max_headers_bytes(), 10);
    assert_eq!(limits.max_params_bytes(), 20);
    assert_eq!(limits.max_nesting_depth(), 3);
}

#[test]
fn from_env_reads_valid_values() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER2_MAX_HEADERS_BYTES, "111");
    env.set(ENV_TIER2_MAX_PARAMS_BYTES, "222");
    env.set(ENV_TIER2_MAX_NESTING_DEPTH, "3");
    let limits = Tier2ResourceLimits::from_env(&env);
    assert_eq!(limits.max_headers_bytes(), 111);
    assert_eq!(limits.max_params_bytes(), 222);
    assert_eq!(limits.max_nesting_depth(), 3);
}

#[test]
fn from_env_falls_back_to_default_on_missing_or_unparseable() {
    let env = InMemoryEnv::new();
    env.set(ENV_TIER2_MAX_HEADERS_BYTES, "not-a-number");
    env.set(ENV_TIER2_MAX_NESTING_DEPTH, "");
    let limits = Tier2ResourceLimits::from_env(&env);
    let defaults = Tier2ResourceLimits::default();
    assert_eq!(limits.max_headers_bytes(), defaults.max_headers_bytes());
    assert_eq!(limits.max_params_bytes(), defaults.max_params_bytes());
    assert_eq!(limits.max_nesting_depth(), defaults.max_nesting_depth());
}

#[test]
fn headers_byte_size_sums_keys_and_values() {
    let mut headers = BTreeMap::new();
    headers.insert("ab".to_string(), "cde".to_string());
    headers.insert("x".to_string(), "yz".to_string());
    assert_eq!(headers_byte_size(headers.iter()), 2 + 3 + 1 + 2);
}

#[test]
fn params_byte_size_matches_serialized_json_length() {
    let value = serde_json::json!({"a": 1});
    let expected = serde_json::to_vec(&value).expect("serialize").len();
    assert_eq!(params_byte_size(&value), expected);
}

#[test]
fn json_nesting_depth_within_limit_does_not_exceed() {
    let value = serde_json::json!({"a": {"b": {"c": 1}}});
    assert!(!json_nesting_depth_exceeds(&value, 3));
}

#[test]
fn json_nesting_depth_at_exact_limit_does_not_exceed() {
    // Build exactly `max_depth` levels of nested arrays: depth counts
    // each Array/Object boundary crossed, so `max_depth` nested arrays
    // reach depth == max_depth without exceeding it.
    let max_depth = 5;
    let mut value = serde_json::json!(1);
    for _ in 0..max_depth {
        value = serde_json::Value::Array(vec![value]);
    }
    assert!(!json_nesting_depth_exceeds(&value, max_depth));
}

#[test]
fn json_nesting_depth_over_limit_exceeds() {
    let max_depth = 5;
    let mut value = serde_json::json!(1);
    for _ in 0..(max_depth + 1) {
        value = serde_json::Value::Array(vec![value]);
    }
    assert!(json_nesting_depth_exceeds(&value, max_depth));
}

#[test]
fn json_nesting_depth_stops_walking_as_soon_as_exceeded() {
    // A 100-level-deep structure with a tiny max_depth must still return
    // promptly (the walker short-circuits via `any` rather than
    // continuing to visit siblings once the depth breach is found).
    let mut value = serde_json::json!("leaf");
    for _ in 0..100 {
        value = serde_json::Value::Array(vec![value]);
    }
    assert!(json_nesting_depth_exceeds(&value, 64));
}
