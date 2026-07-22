use std::collections::BTreeMap;

use super::*;

#[test]
fn accepts_and_orders_kubernetes_labels() {
    let labels = Labels::new(BTreeMap::from([
        ("team.example.com/family".to_string(), "reviewer.v1".to_string()),
        ("app".to_string(), String::new()),
    ]))
    .unwrap();
    assert_eq!(
        labels.as_map().keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["app", "team.example.com/family"]
    );
}

#[test]
fn rejects_invalid_key_prefix_and_value_boundaries() {
    assert!(matches!(
        Labels::new(BTreeMap::from([("UPPER.example/key".to_string(), "ok".to_string())])),
        Err(LabelsError::InvalidKey { .. })
    ));
    assert!(matches!(
        Labels::new(BTreeMap::from([("family".to_string(), "-bad".to_string())])),
        Err(LabelsError::InvalidValue { .. })
    ));
    assert!(matches!(
        Labels::new(BTreeMap::from([("family".to_string(), "a".repeat(64))])),
        Err(LabelsError::InvalidValue { .. })
    ));
    assert!(matches!(
        Labels::new(BTreeMap::from([("a..b/key".to_string(), "ok".to_string())])),
        Err(LabelsError::InvalidKey { .. })
    ));
}

#[test]
fn reports_emptiness_and_supports_try_from() {
    assert!(Labels::new(BTreeMap::new()).unwrap().is_empty());
    let labels: Labels = BTreeMap::from([("app".to_string(), "reviewer".to_string())])
        .try_into()
        .unwrap();
    assert!(!labels.is_empty());
    assert_eq!(labels.get("app"), Some("reviewer"));
}
