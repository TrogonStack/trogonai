#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Upstream-shaped fixtures are written as JSON literals.")
)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;

use serde_json::{Value, json};
use trogon_claude_agent_sdk::upstream::settings::{SettingsAttribution, SettingsPermissionsDefaultMode};
use trogon_claude_agent_sdk::{Settings, SettingsInput};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURES: &str = include_str!("fixtures/upstream_settings.json");

fn fixtures() -> Result<serde_json::Map<String, Value>, serde_json::Error> {
    serde_json::from_str(FIXTURES)
}

fn samples(family: &Value) -> Result<&Vec<Value>, &'static str> {
    family.as_array().ok_or("fixture family is not an array")
}

/// The same value with the keys of every object in reverse order.
fn reversed(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .rev()
                .map(|(key, inner)| (key.clone(), reversed(inner)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(reversed).collect()),
        other => other.clone(),
    }
}

fn reencodes_identically(text: &str) -> TestResult {
    let typed = Settings::from_json(text)?;
    assert_eq!(typed.to_json()?, text, "settings did not re-encode byte-identically");
    Ok(())
}

#[test]
fn every_field_family_round_trips_its_upstream_shaped_fixtures() -> TestResult {
    for (family, group) in fixtures()? {
        for sample in samples(&group)? {
            let typed = Settings::from_value(sample.clone())?;
            assert_eq!(&typed.to_value()?, sample, "{family} did not round-trip");
        }
    }
    Ok(())
}

#[test]
fn every_field_family_re_encodes_byte_identically_in_original_and_reversed_key_order() -> TestResult {
    for (_, group) in fixtures()? {
        for sample in samples(&group)? {
            reencodes_identically(&serde_json::to_string(sample)?)?;
            reencodes_identically(&serde_json::to_string(&reversed(sample))?)?;
        }
    }
    Ok(())
}

#[test]
fn fixtures_cover_every_top_level_key_upstream_declares() -> TestResult {
    let mut covered = BTreeSet::new();
    for (_, group) in fixtures()? {
        for sample in samples(&group)? {
            covered.extend(sample.as_object().ok_or("sample is not an object")?.keys().cloned());
        }
    }
    let declared: BTreeSet<String> = Settings::KNOWN_KEYS.iter().map(|key| (*key).to_owned()).collect();
    assert_eq!(declared.len(), 182);
    assert_eq!(covered, declared);
    Ok(())
}

#[test]
fn every_declared_key_decodes_into_its_own_field_and_none_land_in_extra() -> TestResult {
    for (family, group) in fixtures()? {
        for sample in samples(&group)? {
            let typed = Settings::from_value(sample.clone())?;
            assert!(typed.extra.is_empty(), "{family} left declared keys in extra");
        }
    }
    Ok(())
}

#[test]
fn unknown_top_level_keys_survive_in_place_and_in_order() -> TestResult {
    let text = r#"{"futureSetting":{"b":1,"a":[2,{"d":null,"c":3}]},"model":"opus","zzLast":true,"alsoNew":null}"#;
    let typed = Settings::from_json(text)?;
    assert_eq!(typed.model.as_deref(), Some("opus"));
    assert_eq!(
        typed.extra.keys().map(String::as_str).collect::<Vec<_>>(),
        ["futureSetting", "zzLast", "alsoNew"]
    );
    assert_eq!(typed.extra.get("alsoNew"), Some(&Value::Null));
    assert_eq!(typed.to_json()?, text);
    Ok(())
}

#[test]
fn the_schema_reference_is_a_typed_field() -> TestResult {
    let text = r#"{"$schema":"https://json.schemastore.org/claude-code-settings.json","model":"sonnet"}"#;
    let typed = Settings::from_json(text)?;
    assert_eq!(
        typed.schema.as_deref(),
        Some("https://json.schemastore.org/claude-code-settings.json")
    );
    assert_eq!(typed.to_json()?, text);
    Ok(())
}

#[test]
fn the_permissions_object_keeps_undeclared_keys_and_unlisted_modes() -> TestResult {
    let text = r#"{"permissions":{"allow":["Bash(ls)"],"defaultMode":"acceptEdits","futureRule":{"x":[1]},"deny":["Read(.env)"]}}"#;
    let typed = Settings::from_json(text)?;
    let permissions = typed.permissions.as_ref().ok_or("permissions missing")?;
    assert_eq!(
        permissions.default_mode,
        Some(SettingsPermissionsDefaultMode::AcceptEdits)
    );
    assert_eq!(
        permissions.extra.keys().map(String::as_str).collect::<Vec<_>>(),
        ["futureRule"]
    );
    assert_eq!(typed.to_json()?, text);

    let future = r#"{"permissions":{"defaultMode":"someNewMode"}}"#;
    let typed = Settings::from_json(future)?;
    let permissions = typed.permissions.as_ref().ok_or("permissions missing")?;
    assert_eq!(
        permissions.default_mode,
        Some(SettingsPermissionsDefaultMode::Unknown("someNewMode".to_owned()))
    );
    assert_eq!(typed.to_json()?, future);
    Ok(())
}

#[test]
fn a_union_field_takes_the_member_that_reproduces_it() -> TestResult {
    let flag = Settings::from_json(r#"{"attribution":{"commit":"Co-authored","pr":""}}"#)?;
    assert!(
        !matches!(flag.attribution, Some(SettingsAttribution::Unknown(_))),
        "{:?}",
        flag.attribution
    );
    let odd = r#"{"attribution":[1,"two"]}"#;
    let typed = Settings::from_json(odd)?;
    assert_eq!(typed.attribution, Some(SettingsAttribution::Unknown(json!([1, "two"]))));
    assert_eq!(typed.to_json()?, odd);
    Ok(())
}

#[test]
fn an_empty_document_and_non_object_extra_values_round_trip() -> TestResult {
    assert_eq!(Settings::from_json("{}")?.to_json()?, "{}");
    let text = r#"{"x":1,"y":"two","z":[3],"w":false}"#;
    assert_eq!(Settings::from_json(text)?.to_json()?, text);
    Ok(())
}

#[test]
fn a_known_field_with_the_wrong_json_type_is_an_error_not_a_silent_drop() {
    assert!(Settings::from_json(r#"{"model":7}"#).is_err());
}

#[test]
fn settings_input_names_the_three_upstream_forms() -> TestResult {
    let path = SettingsInput::path("/etc/claude/settings.json");
    assert_eq!(
        path.to_wire_value()?,
        OsString::from("/etc/claude/settings.json"),
        "a path is passed through"
    );
    assert_eq!(path, SettingsInput::Path(PathBuf::from("/etc/claude/settings.json")));

    let text = r#"{ "model": "opus" }"#;
    assert_eq!(SettingsInput::json(text).to_wire_value()?, OsString::from(text));

    let settings = Settings::from_json(r#"{"model":"opus","extraKey":[1,2]}"#)?;
    let inline = SettingsInput::from(settings);
    assert_eq!(
        inline.to_wire_value()?,
        OsString::from(r#"{"model":"opus","extraKey":[1,2]}"#)
    );
    Ok(())
}

#[test]
fn a_python_style_string_is_classified_by_its_braces_after_trimming() {
    assert_eq!(
        SettingsInput::from_wire_string("  {\"model\":\"opus\"}\n"),
        SettingsInput::Json("  {\"model\":\"opus\"}\n".to_owned())
    );
    assert_eq!(
        SettingsInput::from_wire_string("settings.json"),
        SettingsInput::Path(PathBuf::from("settings.json"))
    );
    assert_eq!(
        SettingsInput::from_wire_string("{not closed"),
        SettingsInput::Path(PathBuf::from("{not closed"))
    );
    assert_eq!(SettingsInput::from_wire_string(""), SettingsInput::Path(PathBuf::new()));
}
