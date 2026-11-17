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
    assert_eq!(to_string(&0.5_f32).ok().as_deref(), Some("0.5"));
    assert_eq!(to_string(&0.1_f32).ok().as_deref(), Some("0.10000000149011612"));
}
