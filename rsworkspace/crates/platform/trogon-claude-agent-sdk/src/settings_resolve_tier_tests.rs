use serde_json::{Value, json};

use super::strip_enabling_plugins;
use crate::settings_resolve_values::SettingsMap;

/// What a policy document the user can rewrite keeps of `document` once its plugins are stripped.
fn stripped(document: &Value) -> Option<SettingsMap> {
    let mut document = document.as_object()?.clone();
    strip_enabling_plugins(&mut document);
    Some(document)
}

#[test]
fn a_rewritable_document_that_enables_a_plugin_keeps_only_the_plugins_it_disables() {
    let document = json!({
        "model": "opus",
        "enabledPlugins": {"kept@market": false, "core@builtin": false, "on@market": true},
    });
    let expected = json!({"model": "opus", "enabledPlugins": {"kept@market": false}});
    assert_eq!(stripped(&document), expected.as_object().cloned());
}

#[test]
fn a_rewritable_document_that_disables_no_plugin_but_enables_one_loses_its_plugins() {
    let document = json!({"model": "opus", "enabledPlugins": {"on@market": true, "core@builtin": false}});
    assert_eq!(stripped(&document), json!({"model": "opus"}).as_object().cloned());
}

#[test]
fn a_rewritable_document_that_only_disables_plugins_keeps_them() {
    let document = json!({"enabledPlugins": {"off@market": false, "other@market": false}});
    assert_eq!(stripped(&document), document.as_object().cloned());
}

#[test]
fn a_rewritable_document_whose_plugins_are_not_an_object_keeps_them() {
    let document = json!({"enabledPlugins": ["on@market"]});
    assert_eq!(stripped(&document), document.as_object().cloned());
}
