use super::*;

#[test]
fn validates_model_id() {
    assert_eq!(ModelId::parse("anthropic/opus").unwrap().as_str(), "anthropic/opus");
    assert!(ModelId::parse("").is_err());
    assert!(ModelId::parse("anthropic/opus ").is_err());
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let value = ModelId::parse("anthropic/opus").unwrap();

    assert_eq!(value.as_ref(), "anthropic/opus");
    assert_eq!("anthropic/opus".parse::<ModelId>().unwrap().as_str(), "anthropic/opus");
    assert_eq!(value.to_string(), "anthropic/opus");
}
