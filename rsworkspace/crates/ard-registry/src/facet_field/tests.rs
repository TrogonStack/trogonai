use super::{FacetField, FacetFieldError};

#[test]
fn parses_type() {
    assert_eq!(FacetField::parse("type"), Ok(FacetField::Type));
}

#[test]
fn parses_tags() {
    assert_eq!(FacetField::parse("tags"), Ok(FacetField::Tags));
}

#[test]
fn parses_capabilities() {
    assert_eq!(FacetField::parse("capabilities"), Ok(FacetField::Capabilities));
}

#[test]
fn rejects_unknown_facet() {
    assert!(matches!(
        FacetField::parse("unknown"),
        Err(FacetFieldError::Unsupported(_))
    ));
}

#[test]
fn rejects_empty_facet() {
    assert!(matches!(FacetField::parse(""), Err(FacetFieldError::Unsupported(_))));
}

#[test]
fn as_str_round_trips() {
    assert_eq!(FacetField::Type.as_str(), "type");
    assert_eq!(FacetField::Tags.as_str(), "tags");
    assert_eq!(FacetField::Capabilities.as_str(), "capabilities");
}
