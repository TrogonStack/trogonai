#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "The fixture is the exact JSON the TypeScript SDK writes."
    )
)]

use super::*;

#[test]
fn numbers_are_spelled_like_javascript() {
    let value = serde_json::json!({
        "small": 1,
        "negative": -2,
        "fraction": 1.5,
        "exact": 9_007_199_254_740_992_u64,
        "rounded": 1_152_921_504_606_846_976_u64,
        "negative_rounded": -1_152_921_504_606_846_976_i64,
        "large": 1e21,
        "tiny": 1e-7,
        "mid": 1e20,
        "text": "caf\u{e9}\n",
    });
    assert_eq!(
        to_string(&value).ok().as_deref(),
        Some(
            r#"{"small":1,"negative":-2,"fraction":1.5,"exact":9007199254740992,"rounded":1152921504606847000,"negative_rounded":-1152921504606847000,"large":1e+21,"tiny":1e-7,"mid":100000000000000000000,"text":"café\n"}"#
        )
    );
}

#[test]
fn single_precision_numbers_are_printed_as_the_double_they_widen_to() {
    assert_eq!(to_string(&serde_json::json!(0.5_f32)).ok().as_deref(), Some("0.5"));
    assert_eq!(
        to_string(&serde_json::json!(0.1_f32)).ok().as_deref(),
        Some("0.10000000149011612")
    );
}

/// The expected text is what `JSON.stringify` printed under Node 24 for the object literal with these keys in this
/// order: array indices up to 2^32 - 2 first in ascending order, every other key, `"4294967295"`, `"01"`, `"-1"` and
/// `"1.5"` included, after them in insertion order, at every depth.
#[test]
fn array_index_keys_come_first_like_javascript_objects() -> Result<(), serde_json::Error> {
    let value: Value = serde_json::from_str(
        r#"{"b":1,"1":"one","4294967295":"max","01":"lead","-1":"neg","4294967294":"last","0":"zero","1.5":"frac","n":{"z":1,"2":[{"y":1,"10":2,"9":3}]}}"#,
    )?;
    assert_eq!(
        to_string(&value)?,
        r#"{"0":"zero","1":"one","4294967294":"last","b":1,"4294967295":"max","01":"lead","-1":"neg","1.5":"frac","n":{"2":[{"9":3,"10":2,"y":1}],"z":1}}"#
    );
    Ok(())
}
