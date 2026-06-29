use serde_json::json;

use super::*;

#[test]
fn accepts_url_only() {
    let delivery = UrlOrData::from_optional(Some("https://example.com/card.json".to_owned()), None).unwrap();
    assert!(delivery.url().is_some());
    assert!(delivery.data().is_none());
}

#[test]
fn accepts_data_only() {
    let payload = json!({"name": "assistant"});
    let delivery = UrlOrData::from_optional(None, Some(payload.clone())).unwrap();
    assert!(delivery.url().is_none());
    assert_eq!(delivery.data(), Some(&payload));
}

#[test]
fn rejects_non_object_data() {
    assert_eq!(
        UrlOrData::from_optional(None, Some(json!("not an object"))),
        Err(UrlOrDataError::DataMustBeObject)
    );
}

#[test]
fn rejects_both_url_and_data() {
    assert_eq!(
        UrlOrData::from_optional(
            Some("https://example.com/card.json".to_owned()),
            Some(json!({"name": "assistant"})),
        ),
        Err(UrlOrDataError::BothUrlAndData)
    );
}

#[test]
fn rejects_neither_url_nor_data() {
    assert_eq!(
        UrlOrData::from_optional(None, None),
        Err(UrlOrDataError::NeitherUrlNorData)
    );
}

#[test]
fn rejects_invalid_url() {
    assert!(matches!(
        UrlOrData::from_optional(Some("not a url".to_owned()), None),
        Err(UrlOrDataError::InvalidUrl(_))
    ));
}
