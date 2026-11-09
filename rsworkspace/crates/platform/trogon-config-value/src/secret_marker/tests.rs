use super::*;

#[test]
fn new_accepts_lowercase_identifiers() {
    let marker = SecretMarker::new("gcp", "service-account-1").unwrap();

    assert_eq!(marker.namespace_id(), "gcp");
    assert_eq!(marker.namespace_specific(), "service-account-1");
}

#[test]
fn new_accepts_empty_identifiers() {
    assert_eq!(SecretMarker::new("", "").unwrap(), SecretMarker::default());
}

#[test]
fn new_rejects_invalid_namespace_id() {
    assert_eq!(
        SecretMarker::new("GCP", "key"),
        Err(SecretMarkerError::InvalidNamespaceId)
    );
}

#[test]
fn new_rejects_invalid_namespace_specific() {
    assert_eq!(
        SecretMarker::new("gcp", "key_1"),
        Err(SecretMarkerError::InvalidNamespaceSpecific)
    );
}

#[test]
fn split_separates_marker_from_value() {
    let (marker, value) = SecretMarker::split("secret:svix:whsec:a:b").unwrap();

    assert_eq!(marker, SecretMarker::new("svix", "whsec").unwrap());
    assert_eq!(value, "a:b");
}

#[test]
fn split_ignores_values_outside_the_marker_syntax() {
    for raw in [
        "plain",
        "secret:GCP:key:value",
        "secret:gcp:key:",
        "secret:gcp",
        "secret:gcp_x:key:value",
        "SECRET:gcp:key:value",
    ] {
        assert_eq!(SecretMarker::split(raw), None, "{raw}");
    }
}
