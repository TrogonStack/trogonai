use super::*;

fn host(value: &str) -> String {
    ReleaseHost::new(value).unwrap().as_str().to_string()
}

#[test]
fn rejects_empty_input() {
    assert_eq!(ReleaseHost::new(""), Err(ReleaseHostError::Empty));
}

#[test]
fn rejects_whitespace_only_input() {
    assert_eq!(ReleaseHost::new(" \t\n "), Err(ReleaseHostError::Empty));
}

#[test]
fn rejects_a_lone_root_dot() {
    assert_eq!(ReleaseHost::new("."), Err(ReleaseHostError::Empty));
}

#[test]
fn rejects_input_longer_than_the_host_limit() {
    let value = format!("{}.com", "a".repeat(MAX_HOST_LEN));

    assert_eq!(ReleaseHost::new(&value), Err(ReleaseHostError::TooLong(value.len())));
}

#[test]
fn rejects_a_host_with_a_port() {
    assert_eq!(
        ReleaseHost::new("api.example.com:443"),
        Err(ReleaseHostError::PortNotAllowed)
    );
}

#[test]
fn rejects_a_bracketed_ipv6_literal_with_a_port() {
    assert_eq!(ReleaseHost::new("[::1]:443"), Err(ReleaseHostError::PortNotAllowed));
}

#[test]
fn rejects_a_subdomain_wildcard() {
    assert_eq!(
        ReleaseHost::new("*.example.com"),
        Err(ReleaseHostError::WildcardNotAllowed)
    );
}

#[test]
fn rejects_a_bare_wildcard() {
    assert_eq!(ReleaseHost::new("*"), Err(ReleaseHostError::WildcardNotAllowed));
}

#[test]
fn rejects_an_invalid_character() {
    assert_eq!(
        ReleaseHost::new("api/example.com"),
        Err(ReleaseHostError::InvalidCharacter('/'))
    );
}

#[test]
fn rejects_an_empty_label() {
    assert_eq!(ReleaseHost::new("a..b"), Err(ReleaseHostError::EmptyLabel));
}

#[test]
fn rejects_brackets_around_something_other_than_an_ipv6_address() {
    assert_eq!(
        ReleaseHost::new("[api.example.com]"),
        Err(ReleaseHostError::InvalidIpv6Literal)
    );
}

#[test]
fn lowercases_and_strips_one_trailing_dot() {
    assert_eq!(host("  API.Example.COM.  "), "api.example.com");
}

#[test]
fn canonicalizes_ipv6_to_the_bracketed_form() {
    assert_eq!(host("FE80:0:0::1"), "[fe80::1]");
    assert_eq!(host("[FE80:0:0::1]"), "[fe80::1]");
}

#[test]
fn displays_the_normalized_host() {
    let host = ReleaseHost::new("API.example.com").unwrap();

    assert_eq!(host.to_string(), "api.example.com");
}

#[test]
fn error_messages_name_the_fault() {
    assert_eq!(
        ReleaseHostError::WildcardNotAllowed.to_string(),
        "release host must name one host, not a '*' pattern"
    );
    assert_eq!(
        ReleaseHostError::PortNotAllowed.to_string(),
        "release host must not include a port"
    );
}
