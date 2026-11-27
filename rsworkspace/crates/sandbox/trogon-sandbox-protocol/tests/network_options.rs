use std::sync::Arc;

use trogon_sandbox_contract::{
    DeniedDomain, DeniedDomainReasons, DenyReason, DomainOnlyAllows, DomainPattern, ExternalProxy, Host,
    MitmUnixSocket, NetworkOptions, NetworkPolicy, NoProxy, ParentProxy, ParentProxyUrl, Port, ProxyScheme,
    ResponseHeaderStrip, StrippedHeader, TlsExclusions, TlsInterceptionExemptions, TunnelCap, UnlistedDestinations,
};
use trogon_sandbox_protocol::{
    DEFAULT_TUNNEL_CAP, HostMatch, StandardDenyReason, denied_domain_reason, deny_list_reason,
    domain_only_allows_requires_termination, explain_denied_destination, external_proxy_replaces_runtime_proxy,
    is_loopback_host, is_loopback_name, is_parent_proxy_url, mitm_routes, no_proxy_bypasses, parent_proxy_bypasses,
    parent_proxy_route_for, resolve_parent_proxy, response_strip, select_parent_proxy, tls_exclusions_exclude,
    tls_exemptions_name_host, unlisted_destinations_refuse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn host(input: &str) -> Result<Host, Box<dyn std::error::Error>> {
    Ok(input.parse()?)
}

fn proxy(no_proxy: &str) -> Result<ParentProxy, Box<dyn std::error::Error>> {
    resolve_parent_proxy(Some("http://p:1"), None, Some(no_proxy), |_| None).ok_or_else(|| "no proxy".into())
}

fn bypasses(no_proxy: &str, target: &str) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(parent_proxy_bypasses(&proxy(no_proxy)?, &host(target)?))
}

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    }
}

#[test]
fn nothing_set_means_no_parent_proxy() {
    assert!(resolve_parent_proxy(None, None, None, |_| None).is_none());
    assert!(resolve_parent_proxy(Some(""), None, None, |_| None).is_none());
}

#[test]
fn explicit_settings_win_over_the_environment() -> TestResult {
    let resolved = resolve_parent_proxy(
        Some("http://cfg-proxy:3128"),
        None,
        None,
        env(&[("HTTP_PROXY", "http://env-proxy:8080")]),
    )
    .ok_or("resolved")?;
    let selected = select_parent_proxy(&resolved, ProxyScheme::Http).ok_or("http")?;
    assert_eq!(selected.host(), &host("cfg-proxy")?);
    assert_eq!(selected.port().get(), 3128);
    Ok(())
}

#[test]
fn the_environment_is_the_fallback_and_https_follows_http() -> TestResult {
    let resolved =
        resolve_parent_proxy(None, None, None, env(&[("HTTP_PROXY", "http://env-proxy:8080")])).ok_or("resolved")?;
    for scheme in [ProxyScheme::Http, ProxyScheme::Https] {
        let selected = select_parent_proxy(&resolved, scheme).ok_or("selected")?;
        assert_eq!(selected.host(), &host("env-proxy")?);
    }
    Ok(())
}

#[test]
fn lowercase_environment_variables_are_honoured() -> TestResult {
    let resolved =
        resolve_parent_proxy(None, None, None, env(&[("http_proxy", "http://lower:8080")])).ok_or("resolved")?;
    let selected = select_parent_proxy(&resolved, ProxyScheme::Http).ok_or("http")?;
    assert_eq!(selected.host(), &host("lower")?);
    Ok(())
}

#[test]
fn https_proxy_can_differ_from_http_proxy() -> TestResult {
    let resolved = resolve_parent_proxy(
        None,
        None,
        None,
        env(&[
            ("HTTP_PROXY", "http://plain:8080"),
            ("HTTPS_PROXY", "http://secure:8443"),
        ]),
    )
    .ok_or("resolved")?;
    let plain = select_parent_proxy(&resolved, ProxyScheme::Http).ok_or("http")?;
    let secure = select_parent_proxy(&resolved, ProxyScheme::Https).ok_or("https")?;
    assert_eq!(plain.host(), &host("plain")?);
    assert_eq!(secure.host(), &host("secure")?);
    Ok(())
}

#[test]
fn plain_http_ignores_a_lone_https_proxy() -> TestResult {
    let resolved = resolve_parent_proxy(None, Some("http://secure:1"), None, |_| None).ok_or("resolved")?;
    assert!(select_parent_proxy(&resolved, ProxyScheme::Http).is_none());
    let secure = select_parent_proxy(&resolved, ProxyScheme::Https).ok_or("https")?;
    assert_eq!(secure.host(), &host("secure")?);
    Ok(())
}

#[test]
fn only_http_and_https_proxies_are_accepted() {
    assert!(ParentProxyUrl::new("socks5://proxy:1080").is_err());
    assert!(resolve_parent_proxy(Some("socks5://proxy:1080"), None, None, |_| None).is_none());
}

#[test]
fn an_absolute_url_is_recognised() {
    assert!(is_parent_proxy_url("http://proxy:3128"));
    assert!(!is_parent_proxy_url("/relative/path"));
    assert!(!is_parent_proxy_url("not a url"));
}

#[test]
fn exact_names_match_the_name_and_its_subdomains() -> TestResult {
    assert!(bypasses("example.com", "example.com")?);
    assert!(bypasses("example.com", "api.example.com")?);
    assert!(!bypasses("example.com", "other.com")?);
    assert!(!bypasses("example.com", "notexample.com")?);
    Ok(())
}

#[test]
fn a_leading_dot_matches_the_name_and_its_subdomains_only() -> TestResult {
    assert!(bypasses(".example.com", "api.example.com")?);
    assert!(bypasses(".example.com", "example.com")?);
    assert!(!bypasses(".example.com", "notexample.com")?);
    Ok(())
}

#[test]
fn a_wildcard_prefix_reads_as_a_leading_dot_and_a_bare_star_bypasses_everything() -> TestResult {
    assert!(bypasses("*.local", "foo.local")?);
    assert!(bypasses("*", "anything.com")?);
    Ok(())
}

#[test]
fn cidr_ranges_match_v4_and_v6() -> TestResult {
    assert!(bypasses("10.0.0.0/8,192.168.0.0/16", "10.1.2.3")?);
    assert!(bypasses("10.0.0.0/8,192.168.0.0/16", "192.168.99.1")?);
    assert!(!bypasses("10.0.0.0/8,192.168.0.0/16", "172.16.0.1")?);
    assert!(!bypasses("10.0.0.0/8,192.168.0.0/16", "11.0.0.1")?);
    assert!(bypasses("1.2.3.4/32", "1.2.3.4")?);
    assert!(!bypasses("1.2.3.4/32", "1.2.3.5")?);
    assert!(bypasses("fe80::/10", "fe80::1")?);
    assert!(!bypasses("fe80::/10", "2001:db8::1")?);
    assert!(bypasses("169.254.0.0/16", "169.254.169.254")?);
    assert!(bypasses("0.0.0.0/0", "8.8.8.8")?);
    assert!(!bypasses("10.0.0.0/8", "fe80::1")?);
    assert!(!bypasses("fe80::/10", "10.0.0.1")?);
    Ok(())
}

#[test]
fn loopback_always_bypasses() -> TestResult {
    for target in [
        "localhost",
        "127.0.0.1",
        "127.0.0.2",
        "127.255.255.254",
        "[::1]",
        "foo.localhost",
    ] {
        assert!(bypasses("", target)?, "{target}");
    }
    Ok(())
}

#[test]
fn matching_ignores_case_and_the_entry_port() -> TestResult {
    assert!(bypasses("Example.COM", "EXAMPLE.com")?);
    assert!(bypasses("example.com:8080", "example.com")?);
    Ok(())
}

#[test]
fn an_ipv6_entry_is_not_mangled_by_port_stripping() -> TestResult {
    assert!(bypasses("fe80::1", "fe80::1")?);
    assert!(bypasses("[fe80::1]:8080", "[fe80::1]")?);
    Ok(())
}

#[test]
fn a_zoned_ipv6_entry_is_its_unzoned_range() -> TestResult {
    let no_proxy = "fe80::1%eth0,fe80::%eth1/10,10.0.0.0/8";
    assert!(bypasses(no_proxy, "fe80::1")?);
    assert!(bypasses(no_proxy, "fe80::2")?);
    assert!(bypasses(no_proxy, "10.1.1.1")?);
    assert!(!bypasses(no_proxy, "8.8.8.8")?);
    Ok(())
}

#[test]
fn an_ipv4_mapped_entry_matches_its_ipv4_address_only() -> TestResult {
    let no_proxy = "::ffff:10.0.0.1,::ffff:192.168.0.0/112";
    assert!(bypasses(no_proxy, "10.0.0.1")?);
    assert!(bypasses(no_proxy, "192.168.4.4")?);
    assert!(!bypasses(no_proxy, "10.0.0.2")?);
    assert!(!bypasses(no_proxy, "8.8.8.8")?);
    Ok(())
}

#[test]
fn malformed_cidr_entries_are_ignored_not_widened() -> TestResult {
    assert!(!bypasses("10.0.0.0/", "8.8.8.8")?);
    assert!(!bypasses("10.0.0.0/999,not-an-ip/24", "10.0.0.1")?);
    Ok(())
}

#[test]
fn lists_tolerate_whitespace() -> TestResult {
    let no_proxy = " foo.com , bar.com ,  10.0.0.0/8 ";
    assert!(bypasses(no_proxy, "foo.com")?);
    assert!(bypasses(no_proxy, "bar.com")?);
    assert!(bypasses(no_proxy, "10.1.1.1")?);
    assert!(!bypasses(no_proxy, "baz.com")?);
    Ok(())
}

#[test]
fn an_empty_list_bypasses_nothing() -> TestResult {
    assert!(!no_proxy_bypasses(&NoProxy::new(""), &host("example.com")?));
    Ok(())
}

#[test]
fn the_parent_proxy_is_skipped_for_bypassed_hosts() -> TestResult {
    let resolved = proxy("internal.example")?;
    assert!(parent_proxy_route_for(&resolved, &host("internal.example")?, ProxyScheme::Https).is_none());
    assert!(parent_proxy_route_for(&resolved, &host("public.example")?, ProxyScheme::Https).is_some());
    Ok(())
}

#[test]
fn loopback_names_are_localhost_and_its_subdomains_only() {
    for name in ["localhost", "LOCALHOST", "localhost.", "app.localhost", "a.b.localhost"] {
        assert!(is_loopback_name(name), "{name}");
    }
    for name in [
        "notlocalhost",
        "localhost.example.com",
        "example.com",
        "xlocalhost",
        "host",
    ] {
        assert!(!is_loopback_name(name), "{name}");
    }
}

#[test]
fn loopback_hosts_include_the_whole_loopback_block() -> TestResult {
    assert!(is_loopback_host(&host("127.0.0.9")?));
    assert!(is_loopback_host(&host("127.255.255.254")?));
    assert!(is_loopback_host(&host("[::1]")?));
    assert!(is_loopback_host(&host("app.localhost")?));
    assert!(!is_loopback_host(&host("10.0.0.1")?));
    assert!(!is_loopback_host(&host("example.com")?));
    Ok(())
}

#[test]
fn a_v4_mapped_loopback_is_a_loopback_host() -> TestResult {
    assert!(is_loopback_host(&host("[::ffff:127.0.0.1]")?));
    assert!(is_loopback_host(&host("[::ffff:7f00:1]")?));
    Ok(())
}

#[test]
fn standard_reasons_have_their_fixed_words() -> TestResult {
    let expected = [
        (StandardDenyReason::OnDenyList, "host is on the deny list"),
        (StandardDenyReason::NotOnAllowList, "host is not on the allow list"),
        (StandardDenyReason::UserDenied, "user denied"),
        (StandardDenyReason::MalformedHost, "malformed host"),
    ];
    for (reason, text) in expected {
        assert_eq!(reason.text(), text);
        assert_eq!(reason.reason()?.as_str(), text);
    }
    Ok(())
}

fn denied(entries: &[&str]) -> Result<Vec<DeniedDomain>, Box<dyn std::error::Error>> {
    entries.iter().map(|entry| Ok(DeniedDomain::new(entry)?)).collect()
}

#[test]
fn a_denied_domain_without_text_gets_the_standard_reason() -> TestResult {
    let listed = DeniedDomain::new("a.test")?;
    let other = DeniedDomain::new("b.test")?;
    let reasons = DeniedDomainReasons::new([(listed.clone(), DenyReason::new("blocked by policy")?)]);
    assert_eq!(denied_domain_reason(&reasons, &listed)?.as_str(), "blocked by policy");
    assert_eq!(
        denied_domain_reason(&reasons, &other)?.as_str(),
        "host is on the deny list"
    );
    Ok(())
}

#[test]
fn the_first_matching_denied_domain_explains_the_refusal() -> TestResult {
    let entries = denied(&["*.corp.test", "api.corp.test"])?;
    let reasons = DeniedDomainReasons::new([(entries[0].clone(), DenyReason::new("internal only")?)]);
    let explained = explain_denied_destination(&reasons, &entries, &host("api.corp.test")?, 443)?;
    assert_eq!(explained.ok_or("reason")?.as_str(), "internal only");
    assert!(explain_denied_destination(&reasons, &entries, &host("other.test")?, 443)?.is_none());
    Ok(())
}

#[test]
fn only_a_user_defined_policy_has_a_deny_list_reason() -> TestResult {
    let entries = denied(&["blocked.test"])?;
    let options = NetworkOptions {
        denied_domain_reasons: DeniedDomainReasons::new([(entries[0].clone(), DenyReason::new("no")?)]),
        ..NetworkOptions::default()
    };
    let policy = NetworkPolicy::UserDefined {
        rules: Vec::new(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: entries,
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: Vec::new(),
    };
    let reason = deny_list_reason(&options, &policy, &host("blocked.test")?, 443)?;
    assert_eq!(reason.ok_or("reason")?.as_str(), "no");
    assert!(deny_list_reason(&options, &policy, &host("fine.test")?, 443)?.is_none());
    assert!(deny_list_reason(&options, &NetworkPolicy::AllowAll, &host("blocked.test")?, 443)?.is_none());
    assert!(deny_list_reason(&options, &NetworkPolicy::DenyAll, &host("blocked.test")?, 443)?.is_none());
    Ok(())
}

#[test]
fn tls_exclusions_and_mitm_routes_follow_their_patterns() -> TestResult {
    let exclusions = TlsExclusions::new([DomainPattern::new("*.pinned.test")?]);
    assert!(tls_exclusions_exclude(&exclusions, &host("a.pinned.test")?, 443));
    assert!(!tls_exclusions_exclude(&exclusions, &host("pinned.test")?, 443));
    assert!(!tls_exclusions_exclude(
        &TlsExclusions::default(),
        &host("a.pinned.test")?,
        443
    ));
    let mitm = MitmUnixSocket {
        socket_path: "/run/mitm.sock".into(),
        domains: vec![DomainPattern::new("svc.test:8443")?],
    };
    assert!(mitm_routes(&mitm, &host("svc.test")?, 8443));
    assert!(!mitm_routes(&mitm, &host("svc.test")?, 443));
    Ok(())
}

#[test]
fn tls_exemptions_name_hosts_by_pattern() -> TestResult {
    let exemptions = TlsInterceptionExemptions::new([DomainPattern::new("bank.test")?], []);
    assert!(tls_exemptions_name_host(&exemptions, &host("bank.test")?, 443));
    assert!(!tls_exemptions_name_host(&exemptions, &host("shop.test")?, 443));
    assert!(!tls_exemptions_name_host(
        &TlsInterceptionExemptions::default(),
        &host("bank.test")?,
        443
    ));
    Ok(())
}

#[test]
fn an_external_proxy_replaces_the_runtime_proxy_only_when_both_are_external() -> TestResult {
    let http = Port::new(3128)?;
    let socks = Port::new(1080)?;
    assert!(external_proxy_replaces_runtime_proxy(ExternalProxy::both(http, socks)));
    assert!(!external_proxy_replaces_runtime_proxy(ExternalProxy::http_only(http)));
    assert!(!external_proxy_replaces_runtime_proxy(ExternalProxy::socks_only(socks)));
    Ok(())
}

#[test]
fn the_switches_read_as_their_questions() {
    assert!(domain_only_allows_requires_termination(
        DomainOnlyAllows::RequireTermination
    ));
    assert!(!domain_only_allows_requires_termination(DomainOnlyAllows::PassThrough));
    assert!(unlisted_destinations_refuse(UnlistedDestinations::Refuse));
    assert!(!unlisted_destinations_refuse(UnlistedDestinations::Consult));
    assert!(HostMatch::default().is_required());
    assert!(!HostMatch::NotRequired.is_required());
}

#[test]
fn the_tunnel_cap_default_is_within_the_maximum() -> TestResult {
    assert_eq!(TunnelCap::new(DEFAULT_TUNNEL_CAP)?.get(), 256);
    const { assert!(DEFAULT_TUNNEL_CAP <= TunnelCap::MAX) };
    Ok(())
}

#[test]
fn the_response_strip_is_present_only_when_it_strips_something() -> TestResult {
    let mut options = NetworkOptions::default();
    assert!(response_strip(&options).is_none());
    options.strip_response_headers = ResponseHeaderStrip::new([StrippedHeader::new("Set-Cookie")?]);
    let strip = response_strip(&options).ok_or("strip")?;
    assert!(!strip.is_empty());
    assert!(Arc::strong_count(&strip) == 1);
    Ok(())
}
