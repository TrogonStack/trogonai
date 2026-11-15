use serde_json::Value;

use super::{nests_past_serde_json, parse_json_deep, parses_as_object};
use crate::{
    settings_parse::parse_json, settings_resolve::on_engine_stack, settings_resolve_error::ResolveSettingsError,
};

#[test]
fn a_deep_parse_reads_what_the_shallow_parse_reads() {
    for text in [
        r#"{"a":1,"b":[true,false,null,"x\"y",-0.5e3,1e400],"c":{},"d":[],"a":2}"#,
        " \t\n\r{ \"k\" : [ 1 , { \"\\u0041\" : \"\\ud83d\\ude00\" } ] } \n",
        "{}",
        "[]",
        "\"text\"",
        "12",
        "{\"a\":1,}",
        "[1,]",
        "{\"a\" 1}",
        "{\"a\":1 \"b\":2}",
        "{\"a\":1]",
        "{\"a\":1}x",
        "[01]",
        "[1true]",
        "[+1]",
        "[nul]",
        "{\"a\":\"\u{1}\"}",
        "{\"a\":\"unterminated}",
        "{\"a\":\"ends in a backslash\\",
        "{1:2}",
        "",
        "[",
        "]",
    ] {
        assert_eq!(parse_json_deep(text, 10), parse_json(text).ok(), "{text:?}");
    }
}

#[test]
fn a_deep_parse_reads_past_serde_json_up_to_its_limit() -> Result<(), ResolveSettingsError> {
    on_engine_stack(|| {
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(!nests_past_serde_json(&nested(127)));
        assert!(nests_past_serde_json(&nested(128)));
        assert!(!nests_past_serde_json(&format!("\"{}\"", "[".repeat(200))));
        let mut expected = Value::Array(Vec::new());
        for _ in 1..3000 {
            expected = Value::Array(vec![expected]);
        }
        assert_eq!(parse_json_deep(&nested(3000), 3000), Some(expected));
        assert_eq!(parse_json_deep(&nested(3001), 3000), None);
        Ok(())
    })
}

#[test]
fn an_object_check_reads_any_depth_and_rejects_what_json_parse_rejects() {
    let deep = |opening: &str, closing: &str| format!("{}null{}", opening.repeat(20_000), closing.repeat(20_000));
    assert_eq!(parses_as_object(&deep("{\"x\":", "}")), Some(true));
    assert_eq!(parses_as_object(&format!(" {} ", deep("[", "]"))), Some(false));
    assert_eq!(parses_as_object("{\"a\":[1,{\"b\":\"]\"}],\"c\":{}}"), Some(true));
    for text in ["", "{", "{\"a\":1,}", "[1,]", "{\"a\" 1}", "[1}", "{} x", "{\"a\":1}}"] {
        assert_eq!(parses_as_object(text), None, "{text:?}");
    }
    assert_eq!(
        parses_as_object(&format!("{}null{}", "[".repeat(5), "]".repeat(4))),
        None
    );
}
