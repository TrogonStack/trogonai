use serde_json::{Map, Value, json};

use super::{JsArray, LengthError, StringTooLong, array_length, copy_array_like, fresh_copy, length, settle, union};
use crate::{constants::V8_MAX_STRING_LENGTH, js_array_mark::ArrayMark};

const MARK: ArrayMark = ArrayMark::FALLBACK;

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(entries) => entries,
        _ => Map::new(),
    }
}

fn settled(items: Vec<Value>) -> Value {
    let mut values = [Value::Array(items)];
    assert_eq!(settle(&mut values, MARK), Ok(()));
    let [value] = values;
    value
}

#[test]
fn an_index_past_the_end_leaves_a_run_of_holes_that_later_writes_split() {
    let mut items = vec![json!(1)];
    let mut array = JsArray::new(&mut items, MARK);
    array.set(4, json!(5));
    assert_eq!(array.length(), 5);
    assert_eq!(array.get(2), None);
    array.set(2, json!(3));
    assert_eq!(array.get(2), Some(&json!(3)));
    array.set(0, json!(0));
    assert_eq!(array.length(), 5);
    assert_eq!(items, vec![json!(0), MARK.run(1), json!(3), MARK.run(1), json!(5)]);
    assert_eq!(settled(items), json!([0, null, 3, null, 5]));
}

#[test]
fn a_length_extends_with_holes_and_truncates_through_a_run() {
    let mut items = vec![json!(1), json!(2)];
    let mut array = JsArray::new(&mut items, MARK);
    array.set_length(6);
    assert_eq!(array.length(), 6);
    array.set_length(4);
    assert_eq!(array.length(), 4);
    array.set_length(1);
    assert_eq!(array.length(), 1);
    assert_eq!(items, vec![json!(1)]);
    let mut items = vec![json!(1)];
    let mut array = JsArray::new(&mut items, MARK);
    array.set_length(4);
    array.fill_undefined(2);
    assert_eq!(array.length(), 4);
    array.set_length(2);
    assert_eq!(items, vec![json!(1), MARK.run(1)]);
    let mut items = vec![json!(1)];
    let mut array = JsArray::new(&mut items, MARK);
    array.fill_undefined(3);
    assert_eq!(array.length(), 3);
    assert_eq!(items, vec![json!(1), MARK.run(2)]);
}

#[test]
fn named_properties_sit_after_the_elements_and_never_print() {
    let mut items = vec![json!(1)];
    let mut array = JsArray::new(&mut items, MARK);
    array.set_prop("p", json!({"q": 1}));
    array.set(2, json!(3));
    assert_eq!(array.prop("p"), Some(&json!({"q": 1})));
    if let Some(Value::Object(entries)) = array.prop_mut("p") {
        entries.insert("r".to_owned(), json!(2));
    }
    assert_eq!(array.prop("p"), Some(&json!({"q": 1, "r": 2})));
    assert_eq!(array.length(), 3);
    assert_eq!(length(&items, MARK), 3);
    assert_eq!(settled(items), json!([1, null, 3]));
}

#[test]
fn an_array_like_object_copies_its_indexes_below_its_length() {
    let copy = |value: Value| copy_array_like(&object(value), MARK);
    assert_eq!(
        copy(json!({"length": 3, "2": {"k": 1}, "0": "a", "foo": 1, "01": 9})),
        Ok(Some(vec![json!("a"), MARK.run(1), json!({"k": 1})]))
    );
    assert_eq!(copy(json!({"length": 2})), Ok(Some(vec![MARK.run(2)])));
    assert_eq!(copy(json!({"length": 0, "0": "a"})), Ok(Some(Vec::new())));
    assert_eq!(copy(json!({"length": -0.0})), Ok(Some(Vec::new())));
    for not_array_like in [
        json!({"length": "2"}),
        json!({"length": 1.5}),
        json!({"length": -1}),
        json!({"length": 9_007_199_254_740_992_u64}),
        json!({"0": 1}),
    ] {
        assert_eq!(copy(not_array_like), Ok(None));
    }
    for too_long in [
        json!({"length": 4_294_967_296_u64}),
        json!({"length": 9_007_199_254_740_991_u64}),
    ] {
        assert_eq!(copy(too_long), Err(LengthError::InvalidArrayLength));
    }
}

#[test]
fn a_length_converts_its_value_as_javascript_does() {
    for (value, expected) in [
        (json!(1), Ok(1)),
        (json!(true), Ok(1)),
        (json!(false), Ok(0)),
        (json!(null), Ok(0)),
        (json!(" 0x2 "), Ok(2)),
        (json!("6"), Ok(6)),
        (json!(["1"]), Ok(1)),
        (json!([]), Ok(0)),
        (json!([[[]]]), Ok(0)),
        (json!([null]), Ok(0)),
        (json!([[null, null]]), Err(LengthError::InvalidArrayLength)),
        (json!([true]), Err(LengthError::InvalidArrayLength)),
        (json!([false]), Err(LengthError::InvalidArrayLength)),
        (json!([{}]), Err(LengthError::InvalidArrayLength)),
        (json!(4_294_967_295_u64), Ok(4_294_967_295)),
        (json!("a"), Err(LengthError::InvalidArrayLength)),
        (json!({}), Err(LengthError::InvalidArrayLength)),
        (json!([1, 2]), Err(LengthError::InvalidArrayLength)),
        (json!(1.5), Err(LengthError::InvalidArrayLength)),
        (json!(-1), Err(LengthError::InvalidArrayLength)),
        (json!(4_294_967_296_u64), Err(LengthError::InvalidArrayLength)),
    ] {
        assert_eq!(array_length(&value, MARK), expected, "{value}");
    }
    assert_eq!(array_length(&Value::Array(vec![MARK.run(1)]), MARK), Ok(0));
    assert_eq!(
        array_length(&Value::Array(vec![MARK.run(2)]), MARK),
        Err(LengthError::InvalidArrayLength)
    );
    assert_eq!(
        array_length(&Value::Array(vec![MARK.run(V8_MAX_STRING_LENGTH + 2)]), MARK),
        Err(LengthError::InvalidStringLength)
    );
}

#[test]
fn a_union_keeps_one_undefined_and_drops_named_properties() {
    let left = vec![json!(1), MARK.run(3), json!(1.0), json!({"a": 1}), MARK.props()];
    let right = vec![MARK.run(1), json!({"a": 1}), json!(2), json!(-0.0), json!(0)];
    assert_eq!(
        union(&left, &right, MARK),
        vec![
            json!(1),
            MARK.run(1),
            json!({"a": 1}),
            json!({"a": 1}),
            json!(2),
            json!(-0.0)
        ]
    );
}

#[test]
fn a_fresh_copy_hides_proto_containers_outside_kept_keys() {
    let kept = vec!["modelPicker".to_owned()];
    let value = json!({
        "p": {"__proto__": {"z": 1}, "k": 1},
        "a": [{"__proto__": [1], "k": 1}],
        "s": {"__proto__": 5},
        "modelPicker": {"__proto__": {"z": 1}}
    });
    assert_eq!(
        fresh_copy(&value, &kept, MARK),
        json!({"p": {"k": 1}, "a": [{"k": 1}], "s": {"__proto__": 5}, "modelPicker": {"__proto__": {"z": 1}}})
    );
}

#[test]
fn settling_fails_before_holes_print_longer_than_v8_holds() {
    let fits = V8_MAX_STRING_LENGTH / 4;
    let mut values = [json!({"x": [Value::Array(vec![MARK.run(fits + 1)])]})];
    assert_eq!(settle(&mut values, MARK), Err(StringTooLong));
    assert_eq!(values, [json!({"x": [Value::Array(vec![MARK.run(fits + 1)])]})]);
    assert_eq!(
        settled(vec![json!({"x": [MARK.run(2), MARK.props()]}), MARK.props()]),
        json!([{"x": [null, null]}])
    );
}
