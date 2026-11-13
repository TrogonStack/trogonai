use super::{is_blocked_host, is_ipv6};

#[test]
fn ipv6_literals_follow_node_is_ipv6() {
    for host in ["::", "::1", "1::", "1:2:3:4:5:6:7:8", "::ffff:1.2.3.4", "fe80::1%eth0"] {
        assert!(is_ipv6(host), "{host}");
    }
    for host in [":", ":::", "1:2:3:4:5:6:7:8:9", "[::]", "g::"] {
        assert!(!is_ipv6(host), "{host}");
    }
}

#[test]
fn unspecified_and_loopback_ipv6_hosts_are_blocked() {
    for host in ["[::]", "[::1]", "[::ffff:127.0.0.1]", "[fe80::1]"] {
        assert!(is_blocked_host(host), "{host}");
    }
    assert!(!is_blocked_host("[2001:db8::1]"));
}
