use super::*;

#[test]
fn parses_names_case_insensitively() {
    let host = DestinationHost::parse("Api.Example.com").expect("name");
    assert_eq!(host.to_string(), "api.example.com");
}

#[test]
fn parses_ipv4_literal() {
    assert_eq!(
        DestinationHost::parse("10.1.2.3").expect("ipv4"),
        DestinationHost::Ip("10.1.2.3".parse().expect("addr"))
    );
}

#[test]
fn parses_bracketed_ipv6_and_displays_it_bracketed() {
    let host = DestinationHost::parse("[2001:db8::1]").expect("ipv6");
    assert_eq!(host.to_string(), "[2001:db8::1]");
}

#[test]
fn folds_ipv4_mapped_ipv6_to_ipv4() {
    assert_eq!(
        DestinationHost::parse("[::ffff:127.0.0.1]").expect("mapped"),
        DestinationHost::parse("127.0.0.1").expect("v4")
    );
}

#[test]
fn rejects_bare_ipv6() {
    assert!(matches!(
        DestinationHost::parse("::1"),
        Err(DestinationHostError::Name(_))
    ));
}

#[test]
fn rejects_malformed_brackets() {
    assert_eq!(
        DestinationHost::parse("[::1"),
        Err(DestinationHostError::UnclosedBracket)
    );
    assert_eq!(
        DestinationHost::parse("[example.com]"),
        Err(DestinationHostError::InvalidIpv6)
    );
    assert_eq!(
        DestinationHost::parse("[fe80::1%eth0]"),
        Err(DestinationHostError::InvalidIpv6)
    );
}

#[test]
fn rejects_ipv4_spellings_a_resolver_would_accept() {
    for raw in ["127.1", "0x7f.0.0.1", "2130706433", "010.0.0.1", "1.2.3"] {
        assert!(DestinationHost::parse(raw).is_err(), "{raw:?} must be rejected");
    }
}

#[test]
fn rejects_patterns() {
    for raw in ["*", "*.example.com", "a/b", "a b", ""] {
        assert!(DestinationHost::parse(raw).is_err(), "{raw:?} must be rejected");
    }
}
