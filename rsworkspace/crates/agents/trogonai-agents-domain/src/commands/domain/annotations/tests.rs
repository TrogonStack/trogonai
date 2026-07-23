use std::collections::BTreeMap;

use super::*;

#[test]
fn preserves_opaque_annotation_values_in_key_order() {
    let annotations = Annotations::new(BTreeMap::from([
        ("z.example.com/note".to_string(), " spaces and unicode ✓ ".to_string()),
        ("empty".to_string(), String::new()),
    ]))
    .unwrap();
    assert_eq!(annotations.get("z.example.com/note"), Some(" spaces and unicode ✓ "));
    assert_eq!(annotations.as_map().keys().next().map(String::as_str), Some("empty"));
}

#[test]
fn rejects_invalid_annotation_key() {
    assert!(Annotations::new(BTreeMap::from([("bad/key/again".to_string(), "x".to_string())])).is_err());
}

#[test]
fn reports_emptiness_and_supports_try_from() {
    assert!(Annotations::new(BTreeMap::new()).unwrap().is_empty());
    let annotations: Annotations = BTreeMap::from([("app".to_string(), "reviewer".to_string())])
        .try_into()
        .unwrap();
    assert!(!annotations.is_empty());
    assert_eq!(annotations.get("app"), Some("reviewer"));
}
