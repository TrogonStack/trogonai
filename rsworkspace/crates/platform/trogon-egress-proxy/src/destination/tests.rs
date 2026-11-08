use super::*;

fn parse(raw: &str) -> Result<Destination, DestinationError> {
    raw.parse()
}

#[test]
fn parses_name_ipv4_and_bracketed_ipv6() {
    assert_eq!(
        parse("api.example.com:443").expect("name").to_string(),
        "api.example.com:443"
    );
    assert_eq!(parse("10.0.0.5:8080").expect("ipv4").to_string(), "10.0.0.5:8080");
    assert_eq!(parse("[::1]:443").expect("ipv6").to_string(), "[::1]:443");
}

#[test]
fn name_comparison_ignores_ascii_case() {
    assert_eq!(parse("API.example.com:443"), parse("api.EXAMPLE.com:443"));
}

#[test]
fn port_is_part_of_identity() {
    assert_ne!(parse("example.com:443"), parse("example.com:8443"));
}

#[test]
fn rejects_missing_port() {
    assert_eq!(parse("example.com"), Err(DestinationError::MissingPort));
}

#[test]
fn rejects_zero_and_out_of_range_and_non_digit_ports() {
    assert_eq!(parse("example.com:0"), Err(DestinationError::ZeroPort));
    assert_eq!(parse("example.com:65536"), Err(DestinationError::InvalidPort));
    assert_eq!(parse("example.com:"), Err(DestinationError::InvalidPort));
    assert_eq!(parse("example.com:+443"), Err(DestinationError::InvalidPort));
    assert_eq!(parse("example.com:-1"), Err(DestinationError::InvalidPort));
    assert_eq!(parse("example.com:44 3"), Err(DestinationError::InvalidPort));
    assert_eq!(parse("example.com:https"), Err(DestinationError::InvalidPort));
}

#[test]
fn accepts_the_port_range_edges() {
    assert!(parse("example.com:1").is_ok());
    assert!(parse("example.com:65535").is_ok());
}

#[test]
fn rejects_wildcards_paths_spaces_and_userinfo() {
    for raw in [
        "*:443",
        "*.example.com:443",
        "example.com/x:443",
        "exa mple.com:443",
        " example.com:443",
        "user@example.com:443",
        "http://example.com:443",
    ] {
        assert!(
            matches!(parse(raw), Err(DestinationError::Host(_))),
            "{raw:?} must be rejected"
        );
    }
}

#[test]
fn bare_ipv6_is_not_a_destination() {
    assert!(parse("::1:443").is_err());
    assert!(parse("2001:db8::1:443").is_err());
}

#[test]
fn ipv4_mapped_literal_equals_the_ipv4_literal() {
    assert_eq!(parse("[::ffff:10.0.0.1]:80"), parse("10.0.0.1:80"));
}
