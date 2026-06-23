use super::*;

#[test]
fn accepts_dns_ip_and_port_forms() {
    for raw in [
        "localhost",
        "example.com",
        "api.example.com:8443",
        "127.0.0.1",
        "127.0.0.1:8081",
        "[::1]",
        "[::1]:8081",
    ] {
        assert_eq!(AllowedHost::new(raw).unwrap().as_str(), raw);
    }
}

#[test]
fn rejects_empty_unsafe_or_malformed_hosts() {
    for raw in [
        "",
        "example com",
        "example.com/path",
        "example.com?x=1",
        "example.com#fragment",
        "user@example.com",
        "-example.com",
        "example-.com",
        "example..com",
        "example.com:",
        "example.com:bad",
        "::1",
        "[not-an-ip]",
        "[::1",
        "[::1]bad",
    ] {
        assert!(AllowedHost::new(raw).is_err(), "{raw} should be invalid");
    }
}

#[test]
fn error_display_is_specific() {
    assert_eq!(
        AllowedHostError.to_string(),
        "allowed host must be a DNS name, IP address, or host:port"
    );
}
