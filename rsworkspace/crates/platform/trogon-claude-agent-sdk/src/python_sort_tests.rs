use super::*;
use serde_json::Value as Json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_uniq.json");

fn text<'a>(case: &'a Json, name: &str) -> Result<&'a str, String> {
    case.get(name)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("missing {name}"))
}

fn index(value: &Json) -> Result<usize, String> {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| "bad index".to_owned())
}

fn pairs(case: &Json) -> Result<Vec<(usize, usize)>, String> {
    let list = case
        .get("comparisons")
        .and_then(Json::as_array)
        .ok_or("missing comparisons")?;
    list.iter()
        .map(|pair| {
            let pair = pair.as_array().ok_or("bad pair")?;
            match pair.as_slice() {
                [left, right] => Ok((index(left)?, index(right)?)),
                _ => Err("bad pair".to_owned()),
            }
        })
        .collect()
}

fn items(source: &str) -> Result<Vec<Value>, serde_json::Error> {
    Ok(serde_json::from_str::<Vec<Json>>(source)?
        .iter()
        .map(Value::from)
        .collect())
}

fn cases(name: &str) -> Result<Vec<Json>, String> {
    let fixture: Json = serde_json::from_str(FIXTURE).map_err(|e| e.to_string())?;
    fixture
        .get(name)
        .and_then(Json::as_array)
        .cloned()
        .ok_or_else(|| format!("missing {name}"))
}

#[test]
fn replays_every_python_uniq_case_with_its_comparison_sequence() -> TestResult {
    let cases = cases("uniq_cases")?;
    assert!(cases.len() >= 500);
    let (mut unique, mut duplicated, mut fell_back, mut long) = (0, 0, 0, 0);
    for case in &cases {
        let source = text(case, "items")?;
        let items = items(source)?;
        let expected = pairs(case)?;
        let mut trace = Vec::new();
        let got = uniq_traced(&items, Some(&mut trace));
        let expected_unique = case.get("unique").and_then(Json::as_bool);
        assert_eq!(Some(got), expected_unique, "{source}");
        assert_eq!(uniq(&items), got, "{source}");
        assert_eq!(trace, expected, "{source}");
        let keys: Vec<PyValue> = items.iter().map(|item| unbool(PyValue::from_json(item))).collect();
        let sort_error = sorted_indices(&keys, None).is_err();
        assert_eq!(Some(sort_error), case.get("sort_error").and_then(Json::as_bool));
        if got {
            unique += 1;
        } else {
            duplicated += 1;
        }
        fell_back += usize::from(sort_error);
        long += usize::from(items.len() > 64);
    }
    assert!(unique > 100 && duplicated > 100 && fell_back > 100 && long > 100);
    Ok(())
}

#[test]
fn replays_every_python_sort_case_including_nan_keys() -> TestResult {
    let cases = cases("sort_cases")?;
    assert!(cases.len() >= 300);
    for case in &cases {
        let source = text(case, "items")?;
        let mut keys: Vec<PyValue> = items(source)?.iter().map(PyValue::from_json).collect();
        for position in case.get("nan").and_then(Json::as_array).ok_or("missing nan")? {
            let slot = keys.get_mut(index(position)?).ok_or("nan out of range")?;
            *slot = PyValue::Float(f64::NAN);
        }
        let mut trace = Vec::new();
        let got = sorted_indices(&keys, Some(&mut trace));
        let expected = match case.get("order") {
            Some(Json::Array(order)) => Some(order.iter().map(index).collect::<Result<Vec<usize>, String>>()?),
            _ => None,
        };
        assert_eq!(got.ok(), expected, "{source}");
        assert_eq!(trace, pairs(case)?, "{source}");
    }
    Ok(())
}

fn int(i: i128) -> PyValue {
    PyValue::Int(i)
}

fn list(items: Vec<PyValue>) -> PyValue {
    PyValue::List(items)
}

fn dict(entries: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Dict(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn string(s: &str) -> PyValue {
    PyValue::Str(s.into())
}

#[test]
fn int_float_comparisons_are_exact() {
    let two127 = two_pow_127();
    assert!(int_eq_float(1, 1.0));
    assert!(!int_eq_float(1, 1.5));
    assert!(!int_eq_float(1, f64::NAN));
    assert!(!int_eq_float(1, f64::INFINITY));
    assert!(int_eq_float(i128::MIN, -two127));
    assert!(!int_eq_float(i128::MAX, two127));
    assert!(!int_eq_float(9_007_199_254_740_993, 9_007_199_254_740_992.0));

    assert!(!int_lt_float(5, f64::NAN));
    assert!(!int_lt_float(5, f64::NEG_INFINITY));
    assert!(int_lt_float(5, f64::INFINITY));
    assert!(int_lt_float(i128::MAX, two127));
    assert!(!int_lt_float(i128::MIN, -two127 * 2.0));
    assert!(int_lt_float(5, 5.5));
    assert!(!int_lt_float(5, 5.0));
    assert!(!int_lt_float(5, 4.5));
    assert!(int_lt_float(-6, -5.5));
    assert!(int_lt_float(9_007_199_254_740_992, 9_007_199_254_740_994.0));

    assert!(!float_lt_int(f64::NAN, 5));
    assert!(!float_lt_int(f64::INFINITY, 5));
    assert!(!float_lt_int(two127, i128::MAX));
    assert!(float_lt_int(f64::NEG_INFINITY, 5));
    assert!(float_lt_int(-two127 * 2.0, i128::MIN));
    assert!(float_lt_int(4.5, 5));
    assert!(!float_lt_int(5.0, 5));
    assert!(!float_lt_int(5.5, 5));
    assert!(float_lt_int(-5.5, -5));
}

#[test]
fn numbers_booleans_and_floats_compare_across_types() {
    let float = PyValue::Float;
    assert!(py_eq(&int(1), &float(1.0)));
    assert!(py_eq(&float(1.0), &int(1)));
    assert!(py_eq(&float(0.5), &float(0.5)));
    assert!(py_eq(&PyValue::Bool(true), &int(1)));
    assert!(!py_eq(&PyValue::Bool(true), &int(2)));
    assert_eq!(py_lt(&PyValue::Bool(false), &int(2)), Ok(true));
    assert_eq!(py_lt(&int(2), &float(2.5)), Ok(true));
    assert_eq!(py_lt(&float(2.5), &int(2)), Ok(false));
    assert_eq!(py_lt(&float(1.0), &float(2.0)), Ok(true));
    assert_eq!(py_lt(&int(3), &int(2)), Ok(false));
    assert_eq!(py_lt(&float(f64::NAN), &float(1.0)), Ok(false));
}

#[test]
fn only_numbers_strings_and_lists_order() {
    assert_eq!(py_lt(&string("a"), &string("b")), Ok(true));
    assert_eq!(py_lt(&string("b"), &string("a")), Ok(false));
    assert_eq!(py_lt(&string("\u{e9}"), &string("z")), Ok(false));
    assert_eq!(py_lt(&PyValue::None, &PyValue::None), Err(TypeError));
    assert_eq!(py_lt(&int(1), &string("a")), Err(TypeError));
    assert_eq!(py_lt(&string("a"), &int(1)), Err(TypeError));
    assert_eq!(py_lt(&dict(vec![]), &dict(vec![])), Err(TypeError));
    assert_eq!(
        py_lt(&PyValue::Sentinel(true), &PyValue::Sentinel(true)),
        Err(TypeError)
    );
    assert_eq!(py_lt(&list(vec![]), &int(1)), Err(TypeError));
    assert_eq!(py_lt(&PyValue::Sentinel(false), &int(1)), Err(TypeError));
}

#[test]
fn lists_order_by_the_first_element_that_differs_under_equality() {
    assert_eq!(py_lt(&list(vec![int(1)]), &list(vec![int(1), int(2)])), Ok(true));
    assert_eq!(py_lt(&list(vec![int(1), int(2)]), &list(vec![int(1)])), Ok(false));
    assert_eq!(py_lt(&list(vec![]), &list(vec![])), Ok(false));
    assert_eq!(py_lt(&list(vec![int(1)]), &list(vec![int(2)])), Ok(true));
    assert_eq!(
        py_lt(&list(vec![PyValue::Float(1.0)]), &list(vec![PyValue::Bool(true)])),
        Ok(false)
    );
    assert_eq!(
        py_lt(&list(vec![int(1), string("a")]), &list(vec![int(1), int(3)])),
        Err(TypeError)
    );
    assert_eq!(
        py_lt(&list(vec![int(1), string("a")]), &list(vec![int(2), int(3)])),
        Ok(true)
    );
}

#[test]
fn python_equality_covers_every_value_kind() {
    assert!(py_eq(&PyValue::None, &PyValue::None));
    assert!(!py_eq(&PyValue::None, &int(0)));
    assert!(py_eq(&string("a"), &string("a")));
    assert!(!py_eq(&string("a"), &int(1)));
    assert!(py_eq(&PyValue::Sentinel(true), &PyValue::Sentinel(true)));
    assert!(!py_eq(&PyValue::Sentinel(true), &PyValue::Sentinel(false)));
    assert!(!py_eq(&PyValue::Sentinel(true), &int(1)));
    assert!(py_eq(&list(vec![int(1)]), &list(vec![PyValue::Bool(true)])));
    assert!(!py_eq(&list(vec![int(1)]), &list(vec![int(1), int(1)])));
    assert!(!py_eq(&list(vec![int(1)]), &list(vec![int(2)])));
    assert!(!py_eq(&list(vec![]), &dict(vec![])));
    let ab = dict(vec![("a", int(1)), ("b", int(2))]);
    let ba = dict(vec![("b", int(2)), ("a", int(1))]);
    assert!(py_eq(&ab, &ba));
    assert!(!py_eq(&ab, &dict(vec![("a", int(1))])));
    assert!(!py_eq(&ab, &dict(vec![("a", int(1)), ("b", int(3))])));
    assert!(!py_eq(&ab, &dict(vec![("a", int(1)), ("c", int(2))])));
}

#[test]
fn jsonschema_equal_keeps_booleans_apart_from_numbers() {
    let t = PyValue::Bool(true);
    assert!(equal(&string("a"), &string("a")));
    assert!(!equal(&string("a"), &int(1)));
    assert!(!equal(&int(1), &string("a")));
    assert!(equal(&list(vec![int(1)]), &list(vec![int(1)])));
    assert!(!equal(&list(vec![t.clone()]), &list(vec![int(1)])));
    assert!(!equal(&list(vec![int(1)]), &list(vec![int(1), int(2)])));
    assert!(equal(&dict(vec![("a", t.clone())]), &dict(vec![("a", t.clone())])));
    assert!(!equal(&dict(vec![("a", t.clone())]), &dict(vec![("a", int(1))])));
    assert!(!equal(&dict(vec![("a", int(1))]), &dict(vec![("b", int(1))])));
    assert!(!equal(&dict(vec![("a", int(1))]), &dict(vec![])));
    assert!(equal(&t, &PyValue::Sentinel(true)));
    assert!(equal(&PyValue::Sentinel(false), &PyValue::Bool(false)));
    assert!(!equal(&t, &PyValue::Bool(false)));
    assert!(!equal(&t, &int(1)));
    assert!(!equal(&int(1), &PyValue::Sentinel(true)));
    assert!(equal(&int(1), &PyValue::Float(1.0)));
    assert!(!equal(&list(vec![]), &dict(vec![])));
    assert!(equal(&PyValue::None, &PyValue::None));
}

#[test]
fn unbool_replaces_only_top_level_booleans() {
    assert!(matches!(unbool(PyValue::Bool(true)), PyValue::Sentinel(true)));
    assert!(matches!(unbool(int(1)), PyValue::Int(1)));
    assert!(matches!(unbool(list(vec![PyValue::Bool(true)])), PyValue::List(_)));
}

#[test]
fn jsonschema_uniq_matches_the_documented_examples() -> TestResult {
    assert!(uniq(&items("[]")?));
    assert!(uniq(&items("[1]")?));
    assert!(uniq(&items("[1, true, 0, false]")?));
    assert!(!uniq(&items("[1, 1.0]")?));
    assert!(!uniq(&items("[true, true]")?));
    assert!(!uniq(&items("[[1], \"a\", [1]]")?));
    assert!(uniq(&items("[[1], [true]]")?));
    assert!(!uniq(&items("[{\"a\": 1}, {\"a\": 1}]")?));
    assert!(!uniq(&items("[[{\"a\": 1}], [{\"a\": 1}]]")?));
    Ok(())
}

fn nest(depth: usize, leaf: PyValue) -> PyValue {
    (0..depth).fold(leaf, |inner, _| list(vec![inner]))
}

fn nest_json(depth: usize, leaf: Value) -> Value {
    (0..depth).fold(leaf, |inner, _| Value::Array(vec![inner]))
}

#[test]
fn identity_covers_the_objects_cpython_shares() {
    assert!(identical(&int(-5), &int(-5)));
    assert!(identical(&int(256), &int(256)));
    assert!(!identical(&int(-6), &int(-6)));
    assert!(!identical(&int(257), &int(257)));
    assert!(!identical(&int(1), &int(2)));
    assert!(identical(&string(""), &string("")));
    assert!(identical(&string("a"), &string("a")));
    assert!(identical(&string("\u{e9}"), &string("\u{e9}")));
    assert!(!identical(&string("\u{101}"), &string("\u{101}")));
    assert!(!identical(&string("ab"), &string("ab")));
    assert!(!identical(&string("a"), &string("b")));
    assert!(identical(&PyValue::None, &PyValue::None));
    assert!(identical(&PyValue::Bool(true), &PyValue::Bool(true)));
    assert!(!identical(&PyValue::Bool(true), &PyValue::Bool(false)));
    assert!(identical(&PyValue::Sentinel(false), &PyValue::Sentinel(false)));
    assert!(!identical(&PyValue::Bool(true), &PyValue::Sentinel(true)));
    assert!(!identical(&PyValue::Float(0.0), &PyValue::Float(0.0)));
    assert!(!identical(&list(vec![]), &list(vec![])));
}

#[test]
fn only_list_int_float_str_and_bool_keys_of_one_type_sort_unchecked() {
    assert!(checked(&[]));
    assert!(!checked(&[int(1), int(2)]));
    assert!(!checked(&[PyValue::Float(1.0)]));
    assert!(!checked(&[string("a"), string("b")]));
    assert!(!checked(&[PyValue::Bool(true), PyValue::Bool(false)]));
    assert!(!checked(&[list(vec![]), list(vec![int(1)])]));
    assert!(checked(&[int(1), PyValue::Float(1.0)]));
    assert!(checked(&[dict(vec![]), dict(vec![])]));
    assert!(checked(&[PyValue::None]));
    assert!(checked(&[PyValue::Sentinel(true)]));
}

#[test]
fn nested_list_comparisons_take_one_check_a_level() {
    let keys = [nest(3, int(1)), nest(3, int(2))];
    assert_eq!(sorted_within(&keys, 3), Ok(vec![0, 1]));
    assert_eq!(sorted_within(&keys, 2), Err(SortError::Recursion));
    let keys = [nest(3, int(2)), nest(3, int(1))];
    assert_eq!(sorted_within(&keys, 3), Ok(vec![1, 0]));
    assert_eq!(sorted_within(&keys, 2), Err(SortError::Recursion));
}

#[test]
fn items_cpython_shares_compare_equal_without_a_check() {
    let shared = [list(vec![int(5), int(1)]), list(vec![int(5)])];
    assert_eq!(sorted_within(&shared, 0), Ok(vec![1, 0]));
    let distinct = [list(vec![int(1000), int(1)]), list(vec![int(1000)])];
    assert_eq!(sorted_within(&distinct, 0), Err(SortError::Recursion));
    assert_eq!(sorted_within(&distinct, 1), Ok(vec![1, 0]));
}

#[test]
fn dict_items_compare_their_values_one_check_deeper() {
    let equal_dicts = [
        list(vec![dict(vec![("a", int(1000))]), int(1)]),
        list(vec![dict(vec![("a", int(1000))]), int(2)]),
    ];
    assert_eq!(sorted_within(&equal_dicts, 2), Ok(vec![0, 1]));
    assert_eq!(sorted_within(&equal_dicts, 1), Err(SortError::Recursion));
    let shared_values = [
        list(vec![dict(vec![("a", int(1))]), int(2)]),
        list(vec![dict(vec![("a", int(1))]), int(1)]),
    ];
    assert_eq!(sorted_within(&shared_values, 1), Ok(vec![1, 0]));
    assert_eq!(sorted_within(&shared_values, 0), Err(SortError::Recursion));
    let other_keys = [
        list(vec![dict(vec![("a", int(1))]), int(1)]),
        list(vec![dict(vec![("b", int(1))]), int(2)]),
    ];
    assert_eq!(sorted_within(&other_keys, 1), Err(SortError::Type));
    assert_eq!(sorted_within(&other_keys, 0), Err(SortError::Recursion));
    let other_sizes = [
        list(vec![dict(vec![("a", int(1))])]),
        list(vec![dict(vec![("a", int(1)), ("b", int(2))])]),
    ];
    assert_eq!(sorted_within(&other_sizes, 1), Err(SortError::Type));
    let other_values = [
        list(vec![dict(vec![("a", int(1))])]),
        list(vec![dict(vec![("a", int(2))])]),
    ];
    assert_eq!(sorted_within(&other_values, 2), Err(SortError::Type));
}

#[test]
fn checked_keys_take_a_check_before_their_first_comparison() {
    let dicts = [dict(vec![("a", int(1))]), dict(vec![("a", int(2))])];
    assert_eq!(sorted_within(&dicts, 0), Err(SortError::Recursion));
    assert_eq!(sorted_within(&dicts, 1), Err(SortError::Type));
    let mixed = [int(2), PyValue::Float(1.5)];
    assert_eq!(sorted_within(&mixed, 0), Err(SortError::Recursion));
    assert_eq!(sorted_within(&mixed, 1), Ok(vec![1, 0]));
    let unordered = [list(vec![int(1)]), int(1)];
    assert_eq!(sorted_within(&unordered, 1), Err(SortError::Type));
    let unequal_lengths = [list(vec![list(vec![int(1)])]), list(vec![list(vec![int(1), int(2)])])];
    assert_eq!(sorted_within(&unequal_lengths, 2), Ok(vec![0, 1]));
    assert_eq!(sorted_within(&[], 0), Ok(vec![]));
}

#[test]
fn python_less_than_walks_nested_lists_without_recursing() {
    assert_eq!(py_lt(&nest(1000, int(1)), &nest(1000, int(2))), Ok(true));
    assert_eq!(py_lt(&nest(1000, int(2)), &nest(1000, int(1))), Ok(false));
    assert_eq!(py_lt(&nest(1000, int(1)), &nest(1000, string("a"))), Err(TypeError));
    assert_eq!(py_lt(&list(vec![int(1)]), &list(vec![int(1), int(0)])), Ok(true));
}

#[test]
fn values_nested_past_the_native_stack_convert_compare_and_drop() -> TestResult {
    let depth = 100_000;
    let one = nest_json(depth, Value::from(1));
    let two = nest_json(depth, Value::from(2));
    let keys = [PyValue::from_json(&one), PyValue::from_json(&two)];
    assert!(py_eq(&keys[0], &PyValue::from_json(&one)));
    assert!(!py_eq(&keys[0], &keys[1]));
    assert!(equal(&keys[0], &PyValue::from_json(&one)));
    assert!(!equal(&keys[0], &keys[1]));
    crate::python_json_load::drop_python_deep(one);
    crate::python_json_load::drop_python_deep(two);
    let keys = [nest(3000, int(1)), nest(3000, int(2))];
    assert_eq!(sorted_within(&keys, 3000), Ok(vec![0, 1]));
    assert_eq!(sorted_within(&keys, 2999), Err(SortError::Recursion));
    let object = Value::from(serde_json::from_str::<Json>(r#"{"a": [1, {"b": null}], "c": "x"}"#)?);
    assert!(py_eq(
        &PyValue::from_json(&object),
        &dict(vec![
            ("a", list(vec![int(1), dict(vec![("b", PyValue::None)])])),
            ("c", string("x"))
        ])
    ));
    assert!(matches!(PyValue::from_json(&Value::from(1.5)), PyValue::Float(f) if f.to_bits() == 1.5_f64.to_bits()));
    assert!(matches!(PyValue::from_json(&Value::Null), PyValue::None));
    Ok(())
}

/// `[0, [0, ... [0, leaf]]]`, `depth` lists deep, so two of them first differ at index 1 on every level.
fn chain(depth: usize, leaf: PyValue) -> PyValue {
    (0..depth).fold(leaf, |inner, _| list(vec![int(0), inner]))
}

#[test]
fn deep_lists_order_once_their_first_unequal_members_are_known() {
    let keys = [chain(3, int(1)), chain(3, int(2))];
    assert_eq!(sorted_within(&keys, 3), Ok(vec![0, 1]));
    assert_eq!(sorted_within(&keys, 2), Err(SortError::Recursion));
    let keys = [chain(3, int(2)), chain(3, int(1))];
    assert_eq!(sorted_within(&keys, 3), Ok(vec![1, 0]));
    let depth = crate::constants::C_RECURSION_LIMIT;
    let keys = [chain(depth, int(2)), chain(depth, int(1))];
    assert_eq!(sorted_within(&keys, depth), Ok(vec![1, 0]));
    assert_eq!(sorted_within(&keys, depth - 1), Err(SortError::Recursion));
    let keys = [nest(depth, int(1)), nest(depth, int(2))];
    assert_eq!(sorted_within(&keys, depth), Ok(vec![0, 1]));
    assert_eq!(sorted_within(&keys, depth - 1), Err(SortError::Recursion));
}

/// Each answer below is CPython 3.12's for the same operands.
#[test]
fn ints_outside_i128_compare_and_sort_as_python_ints() -> TestResult {
    let big = |digits: &str| -> Result<PyValue, Box<dyn std::error::Error>> { Ok(PyValue::Big(digits.parse()?)) };
    let two_pow_200 = big("1606938044258990275541962092341162602522202993782792835301376")?;
    let two_pow_127 = big("170141183460469231731687303715884105728")?;
    let below = big("-170141183460469231731687303715884105729")?;
    assert!(py_eq(&two_pow_200, &PyValue::Float(2_f64.powi(200))));
    assert!(!py_eq(&two_pow_127, &int(i128::MAX)));
    assert!(py_eq(&two_pow_200, &two_pow_200.clone()));
    assert!(!py_eq(&two_pow_127, &two_pow_200));
    assert_eq!(py_lt(&int(i128::MAX), &two_pow_127), Ok(true));
    assert_eq!(py_lt(&below, &int(i128::MIN)), Ok(true));
    assert_eq!(py_lt(&two_pow_127, &two_pow_200), Ok(true));
    assert_eq!(py_lt(&below, &two_pow_127), Ok(true));
    assert_eq!(py_lt(&two_pow_200, &PyValue::Float(f64::INFINITY)), Ok(true));
    assert_eq!(py_lt(&PyValue::Float(-1e300), &below), Ok(true));
    assert_eq!(py_lt(&PyValue::Float(f64::NAN), &two_pow_200), Ok(false));
    assert_eq!(py_lt(&two_pow_200, &PyValue::Float(f64::NAN)), Ok(false));
    assert!(!checked(&[int(1), two_pow_127.clone()]));
    assert!(!identical(&two_pow_127, &two_pow_127.clone()));
    let read = |text: &str| crate::python_json_load::loads_text(text, 2);
    let loaded = PyValue::from_json(&read("340282366920938463463374607431768211456")?);
    assert!(matches!(loaded, PyValue::Big(ref n) if n.as_str() == "340282366920938463463374607431768211456"));
    let duplicated =
        read("[1606938044258990275541962092341162602522202993782792835301376, 1.6069380442589903e+60, 1]")?;
    assert!(!uniq(duplicated.as_array().ok_or("an array")?));
    let distinct = read("[1606938044258990275541962092341162602522202993782792835301377, 1.6069380442589903e+60, 1]")?;
    assert!(uniq(distinct.as_array().ok_or("an array")?));
    Ok(())
}
