#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests pin the suggestion shapes claude_agent_sdk 0.2.181 accepts and the messages it raises."
    )
)]

use super::*;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The updates `to_dict` writes back, in its key order.
fn python_dicts(value: Option<&Value>) -> Result<Result<String, String>, serde_json::Error> {
    match python(value) {
        Ok(updates) => {
            let dicts = updates
                .iter()
                .map(PermissionUpdate::to_python_wire_json)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Ok(serde_json::to_string(&dicts)?))
        }
        Err(message) => Ok(Err(message)),
    }
}

#[test]
fn falsy_or_absent_suggestions_are_an_empty_list() -> TestResult {
    assert_eq!(python_dicts(None)?, Ok("[]".to_owned()));
    for value in [
        json!(null),
        json!(false),
        json!(0),
        json!(0.0),
        json!(""),
        json!([]),
        json!({}),
    ] {
        assert_eq!(python_dicts(Some(&value))?, Ok("[]".to_owned()), "{value}");
    }
    Ok(())
}

#[test]
fn suggestions_raise_what_python_raises_while_iterating_them() -> TestResult {
    for (value, message) in [
        (json!(true), "'bool' object is not iterable"),
        (json!(5), "'int' object is not iterable"),
        (json!(1.5), "'float' object is not iterable"),
        (json!("ab"), "'str' object has no attribute 'get'"),
        (json!({"type": "setMode"}), "'str' object has no attribute 'get'"),
        (json!([null]), "'NoneType' object has no attribute 'get'"),
        (json!([true]), "'bool' object has no attribute 'get'"),
        (json!([5]), "'int' object has no attribute 'get'"),
        (json!([1.5]), "'float' object has no attribute 'get'"),
        (json!(["setMode"]), "'str' object has no attribute 'get'"),
        (json!([[]]), "'list' object has no attribute 'get'"),
        (json!([{"destination": "session"}]), "'type'"),
    ] {
        assert_eq!(python_dicts(Some(&value))?, Err(message.to_owned()), "{value}");
    }
    Ok(())
}

#[test]
fn rules_raise_what_python_raises_before_the_type_is_read() -> TestResult {
    for (rules, message) in [
        (json!({"a": 1}), "string indices must be integers, not 'str'"),
        (json!("ab"), "string indices must be integers, not 'str'"),
        (json!(5), "'int' object is not iterable"),
        (json!(1.5), "'float' object is not iterable"),
        (json!(false), "'bool' object is not iterable"),
        (json!(["Bash"]), "string indices must be integers, not 'str'"),
        (json!([[1]]), "list indices must be integers or slices, not str"),
        (json!([null]), "'NoneType' object is not subscriptable"),
        (json!([7]), "'int' object is not subscriptable"),
        (json!([1.5]), "'float' object is not subscriptable"),
        (json!([true]), "'bool' object is not subscriptable"),
        (json!([{"ruleContent": "x"}]), "'toolName'"),
    ] {
        let value = json!([{"rules": rules}]);
        assert_eq!(python_dicts(Some(&value))?, Err(message.to_owned()), "{value}");
    }
    Ok(())
}

#[test]
fn accepted_suggestions_hold_exactly_what_to_dict_writes() -> TestResult {
    for (value, expected) in [
        (
            json!([{"type": "setMode", "mode": "plan", "destination": "session", "extra": 1}]),
            r#"[{"type":"setMode","destination":"session","mode":"plan"}]"#,
        ),
        (
            json!([{"type": "addRules", "rules": {}, "behavior": "allow", "destination": "session"}]),
            r#"[{"type":"addRules","destination":"session","rules":[],"behavior":"allow"}]"#,
        ),
        (
            json!([{"type": "addRules", "rules": "", "behavior": "allow", "destination": "session"}]),
            r#"[{"type":"addRules","destination":"session","rules":[],"behavior":"allow"}]"#,
        ),
        (
            json!([{"type": "replaceRules", "rules": [{"toolName": "Bash"}], "behavior": "deny", "destination": "session"}]),
            r#"[{"type":"replaceRules","destination":"session","rules":[{"toolName":"Bash","ruleContent":null}],"behavior":"deny"}]"#,
        ),
        (
            json!([{"type": "removeRules", "rules": [{"toolName": 5, "ruleContent": 6}], "destination": null}]),
            r#"[{"type":"removeRules","rules":[{"toolName":5,"ruleContent":6}]}]"#,
        ),
        (
            json!([{"type": "addRules", "rules": null, "destination": "session"}]),
            r#"[{"type":"addRules","destination":"session"}]"#,
        ),
        (
            json!([{"type": "setMode", "destination": "session", "rules": [{"toolName": "Bash"}]}]),
            r#"[{"type":"setMode","destination":"session"}]"#,
        ),
        (
            json!([{"type": "addDirectories", "destination": 7, "directories": "x"}]),
            r#"[{"type":"addDirectories","destination":7,"directories":"x"}]"#,
        ),
        (
            json!([{"type": "removeDirectories", "directories": ["/a"], "destination": "cliArg", "mode": "plan"}]),
            r#"[{"type":"removeDirectories","destination":"cliArg","directories":["/a"]}]"#,
        ),
        (
            json!([{"type": "future", "destination": "session", "payload": [1]}]),
            r#"[{"type":"future","destination":"session"}]"#,
        ),
        (json!([{"type": 5}]), r#"[{"type":5}]"#),
    ] {
        assert_eq!(python_dicts(Some(&value))?, Ok(expected.to_owned()), "{value}");
    }
    Ok(())
}

#[test]
fn the_typescript_view_keeps_every_object_and_decodes_the_well_formed_ones() -> TestResult {
    let broken = json!({"destination": "session"});
    let entries = vec![
        json!({"type": "setMode", "mode": "plan", "destination": "session"}),
        broken.clone(),
        json!(5),
        json!("setMode"),
    ];
    let updates = typescript(&entries);
    assert!(matches!(
        updates.as_slice(),
        [PermissionUpdate::SetMode { .. }, PermissionUpdate::Unknown(object)] if Value::Object(object.clone()) == broken
    ));
    Ok(())
}
