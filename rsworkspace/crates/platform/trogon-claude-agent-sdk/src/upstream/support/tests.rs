use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Probe {
    #[serde(default, skip_serializing_if = "Option::is_none", with = "super::double_option")]
    tri: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::optional_value"
    )]
    free: Option<Value>,
}

#[test]
fn omitted_null_and_present_stay_distinct() -> Result<(), serde_json::Error> {
    for fixture in [
        json!({}),
        json!({"tri": null, "free": null}),
        json!({"tri": "x", "free": {"a": 1}}),
    ] {
        let probe: Probe = serde_json::from_value(fixture.clone())?;
        assert_eq!(serde_json::to_value(&probe)?, fixture);
    }
    let null: Probe = serde_json::from_value(json!({"tri": null}))?;
    assert_eq!(null.tri, Some(None));
    let omitted: Probe = serde_json::from_value(json!({}))?;
    assert_eq!(omitted.tri, None);
    Ok(())
}

#[test]
fn tags_match_only_the_listed_literal_of_the_right_json_type() {
    let value = json!({"type": "system", "isReplay": true, "flag": "true"});
    assert!(super::tag_str(&value, "type", &["user", "system"]));
    assert!(!super::tag_str(&value, "type", &["user"]));
    assert!(!super::tag_str(&value, "subtype", &["init"]));
    assert!(super::tag_bool(&value, "isReplay", &[true]));
    assert!(!super::tag_bool(&value, "isReplay", &[false]));
    assert!(!super::tag_bool(&value, "flag", &[true]));
}

#[test]
fn the_candidate_pinning_more_tags_wins_then_fewer_unknown_keys_then_the_earliest() {
    let mut best = None;
    super::keep_most_specific(&mut best, 1, Some((0, "loose")));
    super::keep_most_specific(&mut best, 2, Some((3, "tagged")));
    assert_eq!(best, Some((2, 3, "tagged")));
    super::keep_most_specific(&mut best, 2, Some((1, "closer")));
    assert_eq!(best, Some((2, 1, "closer")));
    super::keep_most_specific(&mut best, 2, Some((1, "later")));
    super::keep_most_specific(&mut best, 1, Some((0, "fewer tags")));
    super::keep_most_specific(&mut best, 3, None);
    assert_eq!(best, Some((2, 1, "closer")));
}
