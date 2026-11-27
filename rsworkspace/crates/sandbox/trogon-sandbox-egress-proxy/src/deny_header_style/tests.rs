use super::*;

#[test]
fn the_default_style_names_its_own_refusals_with_the_default_tag() -> Result<(), Box<dyn std::error::Error>> {
    let line = |tag| DenyHeaderStyle::ProxyError.line(tag);
    for tag in [
        "host-mismatch",
        "method-refused",
        "malformed-request",
        "request-denied",
        "sigv4-refused",
    ] {
        assert_eq!(
            line(tag).as_deref(),
            Some("X-Proxy-Error: blocked-by-egress-proxy"),
            "{tag}"
        );
    }
    assert_eq!(
        line("blocked-by-allowlist").as_deref(),
        Some("X-Proxy-Error: blocked-by-allowlist")
    );
    assert_eq!(
        line("ambiguous-request-target").as_deref(),
        Some("X-Proxy-Error: ambiguous-request-target")
    );
    assert_eq!(line("bad-request"), None);
    Ok(())
}

#[test]
fn a_denial_is_marked_with_the_legacy_style() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        DenyHeaderStyle::ProxyError.denial_line(Some("tea")),
        "X-Proxy-Error: blocked-by-egress-proxy"
    );
    assert_eq!(
        DenyHeaderStyle::DenyReason.denial_line(Some("tea")),
        "X-Deny-Reason: tea"
    );
    assert_eq!(DenyHeaderStyle::DenyReason.denial_line(None), "X-Deny-Reason: denied");
    Ok(())
}

#[test]
fn a_sigv4_refusal_keeps_the_default_tag_under_the_deny_header() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        DenyHeaderStyle::DenyReason.line("sigv4-refused").as_deref(),
        Some("X-Proxy-Error: blocked-by-egress-proxy")
    );
    Ok(())
}

#[test]
fn the_407_carries_no_mark_in_the_legacy_style() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(DenyHeaderStyle::DenyReason.line("unauthenticated"), None);
    assert_eq!(
        DenyHeaderStyle::ProxyError.line("unauthenticated").as_deref(),
        Some("X-Proxy-Error: unauthenticated")
    );
    Ok(())
}

#[test]
fn legacy_codes_replace_the_tags_that_have_one() -> Result<(), Box<dyn std::error::Error>> {
    let line = |tag| DenyHeaderStyle::DenyReason.line(tag);
    assert_eq!(
        line("blocked-by-allowlist").as_deref(),
        Some("X-Deny-Reason: host_not_allowed")
    );
    assert_eq!(
        line("blocked-by-egress-proxy").as_deref(),
        Some("X-Deny-Reason: opaque_tunnel")
    );
    assert_eq!(line("host-mismatch").as_deref(), Some("X-Deny-Reason: misdirected"));
    assert_eq!(
        line("too-many-tunnels").as_deref(),
        Some("X-Deny-Reason: too_many_tunnels")
    );
    assert_eq!(line("method-refused").as_deref(), Some("X-Deny-Reason: method_refused"));
    assert_eq!(line("request-denied").as_deref(), Some("X-Deny-Reason: denied"));
    assert_eq!(line("malformed-request").as_deref(), Some("X-Deny-Reason: bad_request"));
    assert_eq!(
        line("decider_unavailable").as_deref(),
        Some("X-Deny-Reason: decider_unavailable")
    );
    Ok(())
}

#[test]
fn an_upstream_failure_carries_no_mark_in_either_style() -> Result<(), Box<dyn std::error::Error>> {
    for style in [DenyHeaderStyle::DenyReason, DenyHeaderStyle::ProxyError] {
        assert_eq!(style.line("upstream-unreachable"), None);
        assert_eq!(style.line("invalid-upstream-response"), None);
    }
    Ok(())
}
