use std::collections::BTreeMap;

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{SandboxTags, SandboxTagsError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tag_map(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect()
}

#[test]
fn tags_accept_up_to_the_maximum() -> TestResult {
    for count in [0, 1, SandboxTags::MAX_TAGS] {
        let tags = SandboxTags::new(tag_map(count))?;
        assert_eq!(tags.is_empty(), count == 0);
        assert_eq!(tags.iter().count(), count);
        assert_eq!(SandboxTags::try_from(tag_map(count))?, tags);
        assert_eq!(BTreeMap::from(tags), tag_map(count));
    }
    Ok(())
}

#[test]
fn tags_reject_more_than_the_maximum() {
    let expected = SandboxTagsError::TooMany { actual: 6, max: 5 };
    let too_many = tag_map(SandboxTags::MAX_TAGS + 1);
    assert_eq!(SandboxTags::new(too_many.clone()).err(), Some(expected));
    assert_eq!(SandboxTags::try_from(too_many).err(), Some(expected));
    assert_eq!(expected.to_string(), "sandbox supports at most 5 tags, got 6");
}

#[test]
fn tags_serialize_as_a_json_object_of_strings() -> TestResult {
    let tags = SandboxTags::new(BTreeMap::from([
        ("env".to_owned(), "prod".to_owned()),
        ("team".to_owned(), "core".to_owned()),
    ]))?;
    let wire = json!({ "env": "prod", "team": "core" });
    assert_eq!(serde_json::to_value(&tags)?, wire);
    assert_eq!(serde_json::from_value::<SandboxTags>(wire)?, tags);
    assert_eq!(serde_json::to_string(&SandboxTags::default())?, "{}");
    Ok(())
}

#[test]
fn tags_deserialization_rejects_invalid_input() -> TestResult {
    let too_many = serde_json::to_string(&tag_map(SandboxTags::MAX_TAGS + 1))?;
    for json in [too_many.as_str(), r#"{"env":1}"#, "[]", "null"] {
        let actual = serde_json::from_str::<SandboxTags>(json)
            .map(|_| None)
            .unwrap_or_else(|error| Some(error.classify()));
        assert_eq!(actual, Some(Data), "{json}");
    }
    Ok(())
}

#[test]
fn tags_look_up_and_match_filters() -> TestResult {
    let tags = SandboxTags::new(BTreeMap::from([
        ("env".to_owned(), "prod".to_owned()),
        ("team".to_owned(), "core".to_owned()),
    ]))?;
    assert_eq!(tags.get("env"), Some("prod"));
    assert_eq!(tags.get("region"), None);
    assert_eq!(tags.iter().collect::<Vec<_>>(), [("env", "prod"), ("team", "core")]);

    let filter = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    };
    assert!(tags.contains_all(&filter(&[])));
    assert!(tags.contains_all(&filter(&[("env", "prod")])));
    assert!(tags.contains_all(&filter(&[("env", "prod"), ("team", "core")])));
    assert!(!tags.contains_all(&filter(&[("env", "dev")])));
    assert!(!tags.contains_all(&filter(&[("env", "prod"), ("region", "us")])));
    assert!(SandboxTags::default().contains_all(&filter(&[])));
    Ok(())
}
