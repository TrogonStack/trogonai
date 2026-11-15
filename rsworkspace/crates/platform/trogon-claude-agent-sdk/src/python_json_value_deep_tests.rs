use indexmap::IndexMap;
use serde_json::json;

use crate::{
    python_json_value::{PythonJsonValue, PythonNumber, python_json_dumps},
    python_string::PythonString,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Nesting deep enough that a recursive drop, clone, comparison or compact print overflows any thread's stack.
const DEPTH: usize = 200_000;

/// Nesting for the pretty print. Its indentation grows with the depth, so its text grows with the square of it:
/// 2,000 levels print about 64 MB. The derived `Debug` takes several frames a level, so this still overflows
/// [`SMALL_STACK`].
const PRETTY_DEPTH: usize = 2_000;

/// The stack the deep checks run on, an eighth of a spawned thread's default.
const SMALL_STACK: usize = 256 << 10;

/// The shape of [`PythonJsonValue`] with the derived traits the iterative ones replace.
#[derive(Clone, Debug, PartialEq)]
enum Derived {
    Null,
    Bool(bool),
    Number(PythonNumber),
    String(PythonString),
    Array(Vec<Derived>),
    Object(IndexMap<PythonString, Derived>),
}

fn derived(value: &PythonJsonValue) -> Derived {
    match value {
        PythonJsonValue::Null => Derived::Null,
        PythonJsonValue::Bool(flag) => Derived::Bool(*flag),
        PythonJsonValue::Number(number) => Derived::Number(number.clone()),
        PythonJsonValue::String(text) => Derived::String(text.clone()),
        PythonJsonValue::Array(items) => Derived::Array(items.iter().map(derived).collect()),
        PythonJsonValue::Object(members) => Derived::Object(
            members
                .iter()
                .map(|(key, member)| (key.clone(), derived(member)))
                .collect(),
        ),
    }
}

fn samples() -> Vec<PythonJsonValue> {
    let mut samples: Vec<PythonJsonValue> = [
        json!(null),
        json!(true),
        json!(-7),
        json!(1.5),
        json!("a'\"\n\u{1f600}"),
        json!([]),
        json!({}),
        json!([[]]),
        json!([{}]),
        json!({"a": []}),
        json!([null, false, 1, 2.5, "x"]),
        json!({"b": {"c": [1, {"d": "e"}]}, "a": [[], {}, [null]]}),
    ]
    .iter()
    .map(PythonJsonValue::from)
    .collect();
    let mut surrogate = PythonString::new();
    surrogate.push(crate::python_code_point::PythonCodePoint::from(0xd800_u16));
    samples.push(PythonJsonValue::Object(IndexMap::from([(
        surrogate.clone(),
        PythonJsonValue::Array(vec![PythonJsonValue::String(surrogate)]),
    )])));
    samples.push(PythonJsonValue::Number(PythonNumber::Float(f64::NAN)));
    samples
}

/// `leaf` inside `depth` containers, arrays and objects in turn.
fn nest(depth: usize, leaf: PythonJsonValue) -> PythonJsonValue {
    (0..depth).fold(leaf, |inner, at| {
        if at % 2 == 0 {
            PythonJsonValue::Array(vec![PythonJsonValue::Bool(true), inner])
        } else {
            PythonJsonValue::Object(IndexMap::from([("k".into(), inner)]))
        }
    })
}

fn nest_arrays(depth: usize) -> PythonJsonValue {
    (0..depth).fold(PythonJsonValue::Null, |inner, _| PythonJsonValue::Array(vec![inner]))
}

/// Runs `check` on a new thread whose stack is [`SMALL_STACK`].
fn on_small_stack(check: impl FnOnce() + Send + 'static) -> TestResult {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(check)?
        .join()
        .map_err(|_| "the check overflowed or panicked")?;
    Ok(())
}

#[test]
fn debug_prints_what_the_derived_debug_prints() {
    for sample in samples() {
        assert_eq!(format!("{sample:?}"), format!("{:?}", derived(&sample)));
        assert_eq!(format!("{sample:#?}"), format!("{:#?}", derived(&sample)));
    }
}

#[test]
fn equality_and_clone_match_the_derived_traits() {
    let samples = samples();
    for left in &samples {
        let copy = left.clone();
        // NaN is unequal to itself, so the clone is compared by what it prints.
        assert_eq!(format!("{:?}", derived(&copy)), format!("{:?}", derived(left).clone()));
        assert_eq!(python_json_dumps(&copy), python_json_dumps(left));
        for right in &samples {
            assert_eq!(left == right, derived(left) == derived(right), "{left:?} == {right:?}");
        }
    }
}

#[test]
fn objects_compare_as_dicts_and_floats_by_value() {
    let ordered = PythonJsonValue::from(json!({"a": 1, "b": [2, 3]}));
    let reordered = PythonJsonValue::from(json!({"b": [2, 3], "a": 1}));
    assert_eq!(ordered, reordered);
    assert_ne!(
        PythonJsonValue::from(json!([1, 2])),
        PythonJsonValue::from(json!([2, 1]))
    );
    assert_ne!(
        PythonJsonValue::from(json!({"a": 1})),
        PythonJsonValue::from(json!({"a": 1, "b": 2}))
    );
    assert_ne!(
        PythonJsonValue::from(json!({"a": 1})),
        PythonJsonValue::from(json!({"b": 1}))
    );
    assert_ne!(PythonJsonValue::from(json!(1)), PythonJsonValue::from(json!(1.0)));
    let not_a_number = PythonJsonValue::Number(PythonNumber::Float(f64::NAN));
    assert_ne!(not_a_number, not_a_number.clone());
    assert_eq!(
        PythonJsonValue::Number(PythonNumber::Float(0.0)),
        PythonJsonValue::Number(PythonNumber::Float(-0.0))
    );
}

#[test]
fn deep_values_drop_clone_and_compare_on_a_small_stack() -> TestResult {
    on_small_stack(|| {
        let value = nest(DEPTH, PythonJsonValue::Null);
        let copy = value.clone();
        assert!(value == copy);
        let other = nest(DEPTH, PythonJsonValue::Bool(false));
        assert!(value != other);
        drop(value);
        drop(copy);
        drop(other);
    })
}

#[test]
fn deep_values_print_on_a_small_stack() -> TestResult {
    on_small_stack(|| {
        let value = nest_arrays(DEPTH);
        let compact = format!("{value:?}");
        assert_eq!(
            compact,
            format!("{}Null{}", "Array([".repeat(DEPTH), "])".repeat(DEPTH))
        );
        drop(value);
        let value = nest_arrays(PRETTY_DEPTH);
        let pretty = format!("{value:#?}");
        assert!(pretty.starts_with("Array(\n    [\n        Array(\n"));
        assert_eq!(pretty.lines().count(), 4 * PRETTY_DEPTH + 1);

        assert!(pretty.ends_with("    ],\n)"));
    })
}
