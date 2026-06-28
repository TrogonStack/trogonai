use super::*;

#[test]
fn accepts_known_ard_media_types() {
    for value in [
        "application/a2a-agent-card+json",
        "application/mcp-server-card+json",
        "application/ai-catalog+json",
        "application/ai-registry+json",
    ] {
        assert_eq!(MediaType::new(value).unwrap().as_str(), value);
    }
}

#[test]
fn preserves_unknown_future_media_types() {
    let value = "application/vnd.example.custom+proto";
    assert_eq!(MediaType::new(value).unwrap().as_str(), value);
}

#[test]
fn rejects_empty() {
    assert_eq!(MediaType::new(""), Err(MediaTypeError::Empty));
}

#[test]
fn rejects_missing_subtype_separator() {
    assert_eq!(
        MediaType::new("application"),
        Err(MediaTypeError::MissingSubtypeSeparator)
    );
    assert_eq!(MediaType::new("/json"), Err(MediaTypeError::MissingSubtypeSeparator));
}
