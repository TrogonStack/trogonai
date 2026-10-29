use super::*;

#[test]
fn header_names_normalize_to_lowercase() {
    let location = InjectionLocation::header("Authorization").unwrap();

    assert_eq!(location.name(), "authorization");
    assert_eq!(location.to_string(), "header:authorization");
}

#[test]
fn header_and_query_of_the_same_name_are_different_locations() {
    let header = InjectionLocation::header("token").unwrap();
    let query = InjectionLocation::query_parameter("token").unwrap();

    assert_ne!(header, query);

    let locations = InjectionLocations::new([header.clone()]);
    assert!(locations.permits(&header));
    assert!(!locations.permits(&query));
}

#[test]
fn rejects_names_that_would_need_escaping() {
    assert_eq!(InjectionLocation::header(""), Err(InjectionLocationError::Empty));
    assert_eq!(
        InjectionLocation::header("X Api Key"),
        Err(InjectionLocationError::InvalidHeaderCharacter(' '))
    );
    assert_eq!(
        InjectionLocation::header("X-Api-Key\r\nInjected"),
        Err(InjectionLocationError::InvalidHeaderCharacter('\r'))
    );
    assert_eq!(
        InjectionLocation::query_parameter("api key"),
        Err(InjectionLocationError::InvalidQueryCharacter(' '))
    );
    assert_eq!(
        InjectionLocation::body_field("auth..token"),
        Err(InjectionLocationError::EmptyBodyFieldSegment)
    );
}

#[test]
fn body_field_paths_are_dotted_segments() {
    let location = InjectionLocation::body_field("auth.api_key").unwrap();

    assert_eq!(location.to_string(), "body:auth.api_key");
}

#[test]
fn default_permits_no_placement() {
    let locations = InjectionLocations::default();
    let header = InjectionLocation::header("authorization").unwrap();

    assert!(locations.is_empty());
    assert!(!locations.permits(&header));
}
