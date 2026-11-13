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
