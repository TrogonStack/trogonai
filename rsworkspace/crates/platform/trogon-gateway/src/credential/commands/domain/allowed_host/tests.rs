use super::*;

#[test]
fn exact_host_matches_itself_only() {
    let host = AllowedHost::new("api.example.com").unwrap();

    assert!(host.matches("api.example.com"));
    assert!(!host.matches("example.com"));
    assert!(!host.matches("evil.com"));
}

#[test]
fn matching_is_case_insensitive_and_ignores_the_root_dot() {
    let host = AllowedHost::new("API.Example.COM").unwrap();

    assert!(host.matches("api.example.com"));
    assert!(host.matches("API.EXAMPLE.COM"));
    assert!(host.matches("api.example.com."));
}

#[test]
fn port_is_stripped_before_matching() {
    let host = AllowedHost::new("api.example.com").unwrap();

    assert!(host.matches("api.example.com:8443"));
    assert!(!host.matches("api.example.com:notaport"));
}

#[test]
fn wildcard_matches_exactly_one_label() {
    let host = AllowedHost::new("*.example.com").unwrap();

    assert!(host.matches("api.example.com"));
    assert!(!host.matches("example.com"));
    assert!(!host.matches("a.b.example.com"));
}

#[test]
fn wildcard_does_not_match_a_suffix_that_is_not_a_label_boundary() {
    let host = AllowedHost::new("*.example.com").unwrap();

    assert!(!host.matches("notexample.com"));
    assert!(!host.matches("evilexample.com"));
}

#[test]
fn rejects_malformed_patterns() {
    assert_eq!(AllowedHost::new(""), Err(AllowedHostError::Empty));
    assert_eq!(AllowedHost::new("*"), Err(AllowedHostError::BareWildcard));
    assert_eq!(AllowedHost::new("*."), Err(AllowedHostError::BareWildcard));
    assert_eq!(
        AllowedHost::new("api.*.example.com"),
        Err(AllowedHostError::MisplacedWildcard)
    );
    assert_eq!(AllowedHost::new("api..example.com"), Err(AllowedHostError::EmptyLabel));
    assert_eq!(
        AllowedHost::new("api.example.com/path"),
        Err(AllowedHostError::InvalidCharacter('/'))
    );
}

#[test]
fn unrestricted_permits_anything_including_an_absent_host() {
    let hosts = AllowedHosts::Unrestricted;

    assert!(hosts.permits(Some("anything.example.com")));
    assert!(hosts.permits(None));
}

#[test]
fn restricted_denies_an_absent_host() {
    let hosts = AllowedHosts::only(["api.example.com"]).unwrap();

    assert!(hosts.permits(Some("api.example.com")));
    assert!(!hosts.permits(None));
}

#[test]
fn empty_allow_list_denies_everything() {
    let hosts = AllowedHosts::Only(BTreeSet::new());

    assert!(!hosts.permits(Some("api.example.com")));
    assert!(!hosts.permits(None));
}

#[test]
fn round_trips_through_display() {
    assert_eq!(AllowedHost::new("*.example.com").unwrap().to_string(), "*.example.com");
    assert_eq!(
        AllowedHost::new("api.example.com").unwrap().to_string(),
        "api.example.com"
    );
}
