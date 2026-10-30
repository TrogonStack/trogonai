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
fn wildcard_strips_the_candidate_port_before_matching() {
    let host = AllowedHost::new("*.example.com").unwrap();

    assert!(host.matches("api.example.com:8443"));
    assert!(!host.matches("example.com:8443"));
}

#[test]
fn bracketed_ipv6_literal_matches_with_or_without_a_port() {
    let host = AllowedHost::new("[FE80::1]").unwrap();

    assert!(host.matches("[fe80::1]"));
    assert!(host.matches("[fe80::1]:8443"));
    assert!(!host.matches("[fe80::2]"));
}

#[test]
fn bracketed_ipv6_candidate_with_trailing_text_does_not_match() {
    let host = AllowedHost::new("[fe80::1]").unwrap();

    assert!(!host.matches("[fe80::1]evil.example"));
    assert!(!host.matches("[fe80::1]:notaport"));
    assert!(!host.matches("[fe80::1]:"));
    assert!(!host.matches("[fe80::1"));
}

#[test]
fn wildcard_does_not_match_a_candidate_that_is_not_a_valid_host() {
    let host = AllowedHost::new("*.example.com").unwrap();

    assert!(!host.matches("[fe80::1]evil.example.com"));
    assert!(!host.matches("[notipv6]evil.example.com"));
    assert!(!host.matches("a:b.example.com"));
    assert!(!host.matches("a b.example.com"));
    assert!(!host.matches("*.example.com"));
    assert!(host.matches("api.example.com"));
}

#[test]
fn unbracketed_ipv6_candidate_matches_a_bracketed_pattern() {
    let host = AllowedHost::new("[fe80::1]").unwrap();

    assert!(host.matches("fe80::1"));
    assert!(host.matches("FE80:0:0:0:0:0:0:1"));
    assert!(!host.matches("fe80::2"));
}

#[test]
fn unbracketed_ipv6_pattern_matches_the_same_hosts_as_the_bracketed_form() {
    let unbracketed = AllowedHost::new("fe80::1").unwrap();

    assert_eq!(unbracketed, AllowedHost::new("[fe80::1]").unwrap());
    assert_eq!(unbracketed.as_str(), "[fe80::1]");
    assert!(unbracketed.matches("fe80::1"));
    assert!(unbracketed.matches("[fe80::1]"));
    assert!(unbracketed.matches("[fe80::1]:8443"));
    assert!(!unbracketed.matches("fe80::"));
}

#[test]
fn ipv6_literals_are_compared_in_canonical_form() {
    let host = AllowedHost::new("[0:0:0:0:0:0:0:1]").unwrap();

    assert!(host.matches("::1"));
    assert!(host.matches("[::1]:443"));
}

#[test]
fn name_with_port_still_matches_after_ipv6_handling() {
    let host = AllowedHost::new("example.com").unwrap();

    assert!(host.matches("example.com:443"));
    assert!(AllowedHost::new("127.0.0.1").unwrap().matches("127.0.0.1:8080"));
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
fn rejects_a_port_in_the_pattern() {
    assert_eq!(
        AllowedHost::new("api.example.com:8443"),
        Err(AllowedHostError::PortNotAllowed)
    );
    assert_eq!(
        AllowedHost::new("*.example.com:443"),
        Err(AllowedHostError::PortNotAllowed)
    );
    assert_eq!(
        AllowedHost::new("127.0.0.1:8080"),
        Err(AllowedHostError::PortNotAllowed)
    );
    assert_eq!(AllowedHost::new("[::1]:8443"), Err(AllowedHostError::PortNotAllowed));
    assert_eq!(
        AllowedHost::new("api.example.com:https"),
        Err(AllowedHostError::InvalidCharacter(':'))
    );
}

#[test]
fn rejects_malformed_ipv6_literals() {
    assert_eq!(AllowedHost::new("[::1"), Err(AllowedHostError::InvalidIpv6Literal));
    assert_eq!(AllowedHost::new("[]"), Err(AllowedHostError::InvalidIpv6Literal));
    assert_eq!(
        AllowedHost::new("[not-an-address]"),
        Err(AllowedHostError::InvalidIpv6Literal)
    );
    assert_eq!(
        AllowedHost::new("[::1]extra"),
        Err(AllowedHostError::InvalidIpv6Literal)
    );
    assert_eq!(
        AllowedHost::new("fe80::zz"),
        Err(AllowedHostError::InvalidCharacter(':'))
    );
    assert_eq!(
        AllowedHost::new("*.[::1]"),
        Err(AllowedHostError::InvalidCharacter('['))
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
