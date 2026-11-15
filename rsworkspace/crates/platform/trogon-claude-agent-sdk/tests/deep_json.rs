//! `DeepJsonValue` holds a member nested past what a recursive walk takes on a small native stack, and drops,
//! clones, compares, prints and writes it with loops over explicit stacks.

use std::thread;

use serde_json::{Map, Value};
use trogon_claude_agent_sdk::{DeepJsonValue, Settings};

type Failure = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), Failure>;

/// The nesting of the deep member: arrays and objects taking turns, with the leaf inside the last.
const DEPTH: usize = 9_997;

/// The stack Rust gives a spawned thread by default, where the deep member must drop without overflowing it.
const SMALL_STACK_BYTES: usize = 2 * 1024 * 1024;

/// Room for `serde_json`, which decodes `Settings` through a recursive `serde_json::Value`.
const DEEP_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Whether the container `outer` levels inside the outermost one is an array, or else an object under the key `a`.
fn is_array(outer: usize) -> bool {
    outer.is_multiple_of(2)
}

/// The deep member as JSON text.
fn deep_text() -> String {
    let mut text = String::new();
    for outer in 0..DEPTH {
        text.push_str(if is_array(outer) { "[" } else { "{\"a\":" });
    }
    text.push_str("\"leaf\"");
    for outer in (0..DEPTH).rev() {
        text.push(if is_array(outer) { ']' } else { '}' });
    }
    text
}

/// The deep member as `serde_json::Value` prints it with `{:?}`.
fn deep_debug() -> String {
    let mut text = String::new();
    for outer in 0..DEPTH {
        text.push_str(if is_array(outer) { "Array [" } else { "Object {\"a\": " });
    }
    text.push_str("String(\"leaf\")");
    for outer in (0..DEPTH).rev() {
        text.push(if is_array(outer) { ']' } else { '}' });
    }
    text
}

/// The deep member around `leaf`, built from the leaf outward without recursion and held before it can drop.
fn deep_value_around(leaf: &str) -> DeepJsonValue {
    let mut value = DeepJsonValue::from(Value::String(leaf.to_owned()));
    for outer in (0..DEPTH).rev() {
        let inner = value.into_value();
        value = DeepJsonValue::from(if is_array(outer) {
            Value::Array(vec![inner])
        } else {
            Value::Object(Map::from_iter([("a".to_owned(), inner)]))
        });
    }
    value
}

fn deep_value() -> DeepJsonValue {
    deep_value_around("leaf")
}

/// A settings document holding the deep member under the key `x`, which the upstream declarations do not list.
fn settings_value() -> DeepJsonValue {
    let member = deep_value().into_value();
    DeepJsonValue::from(Value::Object(Map::from_iter([("x".to_owned(), member)])))
}

fn on_small_stack(work: impl FnOnce() -> TestResult + Send + 'static) -> TestResult {
    let worker = thread::Builder::new().stack_size(SMALL_STACK_BYTES).spawn(work)?;
    worker.join().map_err(|_| "the small stack worker panicked")?
}

#[test]
fn a_value_nested_9997_levels_drops_clones_compares_prints_and_writes_on_a_2_mib_stack() -> TestResult {
    let text = deep_text();
    let debug = deep_debug();
    let held = deep_value();
    let other = deep_value();
    on_small_stack(move || {
        let copy = held.clone();
        assert!(copy == held);
        assert!(copy == other);
        assert!(copy == *other.as_value());
        assert_eq!(held.to_json()?, text);
        assert_eq!(format!("{held:?}"), debug);
        drop((copy, held, other));
        Ok(())
    })
}

#[test]
fn values_that_differ_only_at_the_leaf_compare_unequal_on_a_2_mib_stack() -> TestResult {
    let held = deep_value();
    let other = deep_value_around("left");
    on_small_stack(move || {
        assert!(other != held);
        assert!(other != *held.as_value());
        drop((held, other));
        Ok(())
    })
}

#[test]
fn settings_holding_a_member_nested_9997_levels_encode_it_byte_for_byte_on_a_2_mib_stack() -> TestResult {
    let decoder = thread::Builder::new()
        .stack_size(DEEP_STACK_BYTES)
        .spawn(|| -> Result<Settings, Failure> {
            Settings::from_value(settings_value().into_value()).map_err(Failure::from)
        })?;
    let settings = decoder.join().map_err(|_| "the settings decoder panicked")??;
    let text = format!("{{\"x\":{}}}", deep_text());
    on_small_stack(move || {
        assert!(settings.extra.get("x").is_some_and(|member| *member == deep_value()));
        let copy = settings.clone();
        assert!(copy == settings);
        assert_eq!(serde_json::to_string(&settings)?, text);
        assert_eq!(settings.to_json()?, text);
        assert!(DeepJsonValue::encode(&settings)? == settings_value());
        assert!(settings.to_value().is_ok_and(|value| {
            let deep = DeepJsonValue::from(value);
            deep == settings_value()
        }));
        drop((copy, settings));
        Ok(())
    })
}
