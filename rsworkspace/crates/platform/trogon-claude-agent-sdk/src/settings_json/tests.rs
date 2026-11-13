use serde_json::{Value, json};

use super::{js_normalize, js_stringify, python_dumps};

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

#[test]
fn javascript_orders_index_keys_first_and_prints_whole_floats_plainly() {
    let value = parse(r#"{"b":1,"2":true,"a":1.0,"1":[1.0,2.50],"x":"\u0001\u2028é"}"#);
    assert_eq!(
        js_stringify(&value),
        "{\"1\":[1,2.5],\"2\":true,\"b\":1,\"a\":1,\"x\":\"\\u0001\u{2028}é\"}"
    );
}

#[test]
fn javascript_switches_to_exponents_at_the_same_thresholds() {
    let value = parse(r#"[1e21,1.5e-7,123456789012345680000,9007199254740993,-0.0,0.000001]"#);
    assert_eq!(
        js_stringify(&value),
        "[1e+21,1.5e-7,123456789012345680000,9007199254740992,0,0.000001]"
    );
}

#[test]
fn normalizing_matches_the_stringified_form() {
    let value = parse(r#"{"b":1,"2":1.0}"#);
    assert_eq!(js_normalize(&value).to_string(), "{\"2\":1,\"b\":1}");
}

#[test]
fn python_separates_with_spaces_and_escapes_non_ascii() {
    let value = json!({"a": [1, 2.0, "é\u{7f}\u{1f600}"], "b": {"c": null}, "d": 1e16, "e": 1e-7});
    assert_eq!(
        python_dumps(&value),
        "{\"a\": [1, 2.0, \"\\u00e9\\u007f\\ud83d\\ude00\"], \"b\": {\"c\": null}, \"d\": 1e+16, \"e\": 1e-07}"
    );
}
