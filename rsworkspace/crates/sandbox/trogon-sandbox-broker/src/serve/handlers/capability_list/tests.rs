use super::*;

#[test]
fn an_empty_body_asks_for_the_default_listing() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(capability_request(b""), Some(ListCapabilitiesRequest::default()));
    Ok(())
}

#[test]
fn a_body_is_decoded() -> Result<(), Box<dyn std::error::Error>> {
    let body = serde_json::to_vec(&ListCapabilitiesRequest::default())?;
    assert_eq!(capability_request(&body), Some(ListCapabilitiesRequest::default()));
    Ok(())
}

#[test]
fn an_undecodable_body_is_dropped() {
    assert!(capability_request(b"not json").is_none());
}
