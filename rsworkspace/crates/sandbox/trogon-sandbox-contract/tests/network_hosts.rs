use std::net::IpAddr;

use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    DeniedDomain, DomainName, DomainPattern, DomainPatternError, Host, HostError, PatternTarget, Port, PortError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: DeserializeOwned>(value: serde_json::Value) -> TestResult {
    let from_value = error_of(serde_json::from_value::<T>(value.clone()))?;
    assert_eq!(from_value.classify(), Data);
    let from_str = error_of(serde_json::from_str::<T>(&value.to_string()))?;
    assert_eq!(from_str.classify(), Data);
    Ok(())
}

fn host(value: &str) -> Result<Host, Box<dyn std::error::Error>> {
    Ok(Host::new(value)?)
}

fn pattern(value: &str) -> Result<DomainPattern, Box<dyn std::error::Error>> {
    Ok(DomainPattern::new(value)?)
}

fn domain_name(value: &str) -> Result<DomainName, Box<dyn std::error::Error>> {
    match host(value)? {
        Host::Domain(name) => Ok(name),
        Host::Ip(_) => Err("expected a domain".into()),
    }
}

#[test]
fn a_port_is_one_to_65535() -> TestResult {
    let port = Port::new(443)?;
    assert_eq!(port.get(), 443);
    assert_eq!(u16::from(port), 443);
    assert_eq!(port.to_string(), "443");
    assert_eq!(Port::try_from(65535)?.get(), 65535);
    assert_eq!(serde_json::to_value(port)?, json!(443));
    assert_eq!(serde_json::from_value::<Port>(json!(443))?, port);
    assert_eq!(serde_json::from_str::<Port>("443")?, port);

    let zero = error_of(Port::new(0))?;
    assert_eq!(zero.input(), "0");
    assert_eq!(zero.to_string(), "port '0' is not in 1-65535 without leading zeros");
    assert_eq!(error_of(Port::try_from(0))?, zero);
    rejects::<Port>(json!(0))?;
    Ok(())
}

#[test]
fn a_port_parses_only_plain_digits_without_leading_zeros() -> TestResult {
    for (text, number) in [("1", 1), ("80", 80), ("443", 443), ("65535", 65535)] {
        assert_eq!(text.parse::<Port>()?.get(), number);
    }
    for text in [
        "0", "0443", "65536", "99999", "123456", "", "abc", "+1", "-1", " 1", "1 ",
    ] {
        let error: PortError = error_of(text.parse::<Port>())?;
        assert_eq!(error.input(), text);
    }
    Ok(())
}

#[test]
fn a_host_is_canonical() -> TestResult {
    for (input, canonical) in [
        ("registry.npmjs.org", "registry.npmjs.org"),
        ("sub-domain.example.co.uk", "sub-domain.example.co.uk"),
        ("Api.Example.com.", "api.example.com"),
        ("_dmarc.example.com", "_dmarc.example.com"),
        ("_acme-challenge.example.com", "_acme-challenge.example.com"),
        ("192.168.1.1", "192.168.1.1"),
        ("127.1", "127.0.0.1"),
        ("2130706433", "127.0.0.1"),
        ("0x7f.0.0.1", "127.0.0.1"),
        ("2852039166", "169.254.169.254"),
        ("::1", "[::1]"),
        ("[::1]", "[::1]"),
        ("0:0:0:0:0:0:0:1", "[::1]"),
        ("fe80::1", "[fe80::1]"),
        ("::ffff:169.254.169.254", "169.254.169.254"),
        ("[::ffff:169.254.169.254]", "169.254.169.254"),
        ("B\u{fc}cher.example", "xn--bcher-kva.example"),
        ("xn--bcher-kva.example", "xn--bcher-kva.example"),
    ] {
        let parsed = host(input)?;
        assert_eq!(parsed.to_string(), canonical, "{input}");
        assert_eq!(parsed, host(canonical)?, "{input}");
        assert_eq!(input.parse::<Host>()?, parsed);
        assert_eq!(Host::try_from(input.to_string())?, parsed);
        assert_eq!(String::from(parsed), canonical);
    }
    Ok(())
}

#[test]
fn a_host_rejects_what_the_resolver_would_read_differently() -> TestResult {
    assert_eq!(error_of(Host::new(""))?, HostError::Empty);
    assert_eq!(error_of(Host::new(""))?.to_string(), "host must not be empty");
    let overlong_label = format!("{}.com", "a".repeat(64));
    let overlong_name = "a".repeat(256);
    let many_labels = vec!["a".repeat(60); 5].join(".");
    let too_many_labels = format!("{}.com", "a.".repeat(130));
    for input in [
        "evil.com\u{0}.allowed.com",
        "evil.com%00.allowed.com",
        "a b.com",
        "evil.com/",
        "evil.com:443",
        "a..b.com",
        "-a.example.com",
        "a-.example.com",
        ".",
        "a\r\nInjected: x",
        "a\nInjected: x",
        "a\u{0}.example.com",
        "fe80::1%eth0",
        "fe80::1%en0",
        "[fe80::1%eth0]",
        "::ffff:127.0.0.1%x.github.com",
        "::ffff:1.2.3.4%x.allowed.com",
        overlong_label.as_str(),
        overlong_name.as_str(),
        many_labels.as_str(),
        too_many_labels.as_str(),
    ] {
        assert_eq!(
            error_of(Host::new(input))?,
            HostError::Invalid(input.to_string()),
            "{input:?}"
        );
    }
    assert_eq!(
        error_of(Host::new("a b.com"))?.to_string(),
        "host 'a b.com' is not a valid host name or IP address"
    );
    Ok(())
}

#[test]
fn a_host_built_from_an_ip_address_is_canonical() -> TestResult {
    let v4 = IpAddr::from([203, 0, 113, 7]);
    assert_eq!(Host::from(v4), Host::Ip(v4));
    let v6: IpAddr = "2001:db8::1".parse()?;
    assert_eq!(Host::from(v6), Host::Ip(v6));
    let mapped: IpAddr = "::ffff:203.0.113.7".parse()?;
    assert_eq!(Host::from(mapped), Host::Ip(v4));
    Ok(())
}

#[test]
fn a_host_serializes_as_its_canonical_string() -> TestResult {
    let parsed = host("Api.Example.com")?;
    assert_eq!(serde_json::to_value(&parsed)?, json!("api.example.com"));
    assert_eq!(serde_json::from_value::<Host>(json!("API.example.com"))?, parsed);
    assert_eq!(serde_json::from_str::<Host>("\"API.example.com\"")?, parsed);
    assert_eq!(serde_json::to_value(host("::1")?)?, json!("[::1]"));
    rejects::<Host>(json!("a b.com"))?;
    rejects::<Host>(json!(""))?;
    Ok(())
}

#[test]
fn a_domain_name_is_a_canonical_dns_name() -> TestResult {
    let name = domain_name("Example.COM.")?;
    assert_eq!(name.as_str(), "example.com");
    assert_eq!(name.to_string(), "example.com");
    assert_eq!(DomainName::MAX_LEN, 253);
    assert_eq!(DomainName::MAX_LABEL_LEN, 63);
    Ok(())
}

#[test]
fn a_pattern_accepts_names_wildcards_and_ip_literals_with_an_optional_port() -> TestResult {
    for input in [
        "example.com",
        "example.com:443",
        "example.com:1",
        "example.com:65535",
        "[::1]",
        "[::1]:443",
        "[2001:db8::1]:8443",
        "*.example.com:8080",
        "localhost:3000",
        "localhost",
        "*.github.com",
        "*.co.uk",
        "10.0.0.1:5432",
    ] {
        assert_eq!(pattern(input)?.to_string(), input);
        assert_eq!(input.parse::<DomainPattern>()?, pattern(input)?);
        assert_eq!(DomainPattern::try_from(input.to_string())?, pattern(input)?);
    }
    Ok(())
}

#[test]
fn a_pattern_canonicalizes_its_host_and_drops_an_ipv6_zone() -> TestResult {
    for (input, canonical) in [
        ("Example.COM", "example.com"),
        ("*.Example.com:443", "*.example.com:443"),
        ("[2001:DB8:0:0::1]:8443", "[2001:db8::1]:8443"),
        ("[fe80::1%en0]:22", "[fe80::1]:22"),
        ("127.1", "127.0.0.1"),
        ("*.b\u{fc}cher.example", "*.xn--bcher-kva.example"),
    ] {
        let parsed = pattern(input)?;
        assert_eq!(parsed.to_string(), canonical);
        assert_eq!(String::from(parsed.clone()), canonical);
        assert_eq!(pattern(canonical)?, parsed);
    }
    Ok(())
}

#[test]
fn a_pattern_exposes_its_target_and_port() -> TestResult {
    assert_eq!(pattern("example.com:443")?.port(), Some(Port::new(443)?));
    assert_eq!(pattern("*.example.com:8080")?.port(), Some(Port::new(8080)?));
    assert_eq!(pattern("example.com")?.port(), None);
    assert_eq!(
        pattern("example.com")?.target(),
        &PatternTarget::Exact(host("example.com")?)
    );
    assert_eq!(pattern("[::1]")?.target(), &PatternTarget::Exact(host("::1")?));
    assert_eq!(
        pattern("*.example.com")?.target(),
        &PatternTarget::Subdomains(domain_name("example.com")?)
    );
    Ok(())
}

#[test]
fn a_pattern_rejects_malformed_input_with_a_reason() -> TestResult {
    assert_eq!(error_of(DomainPattern::new(""))?, DomainPatternError::Empty);
    for input in ["*", "*:22"] {
        assert_eq!(
            error_of(DomainPattern::new(input))?,
            DomainPatternError::BareWildcardNotAllowed
        );
    }
    for input in ["::1", "fd00:ec2::254", "2001:db8::1:443", "example.com:80:443"] {
        assert_eq!(
            error_of(DomainPattern::new(input))?,
            DomainPatternError::UnbracketedIpv6(input.to_string()),
            "{input}"
        );
    }
    for input in ["[::1", "[::1]junk", "[not-an-ip]", "[10.0.0.1]"] {
        assert_eq!(
            error_of(DomainPattern::new(input))?,
            DomainPatternError::MalformedBrackets(input.to_string()),
            "{input}"
        );
    }
    for input in [
        "*example.com",
        "*.com",
        "*.",
        "*.*.example.com",
        "foo.*.com",
        "a*b.com",
        "*.10.0.0.1",
        "*.2.3.4",
    ] {
        assert_eq!(
            error_of(DomainPattern::new(input))?,
            DomainPatternError::InvalidWildcard(input.to_string()),
            "{input}"
        );
    }
    assert_eq!(
        error_of(DomainPattern::new("not-a-domain"))?,
        DomainPatternError::SingleLabel("not-a-domain".to_string())
    );
    for input in [
        "a..b.com",
        "-a.example.com",
        "exa mple.com",
        "evil%2ecom.test",
        "evil.com\u{0}.allowed.com",
    ] {
        assert_eq!(
            error_of(DomainPattern::new(input))?,
            DomainPatternError::Host(HostError::Invalid(input.to_string())),
            "{input:?}"
        );
    }
    Ok(())
}

#[test]
fn a_pattern_rejects_a_bad_port_suffix() -> TestResult {
    for (input, port) in [
        ("example.com:0", "0"),
        ("example.com:65536", "65536"),
        ("example.com:99999999", "99999999"),
        ("example.com:0443", "0443"),
        ("example.com:abc", "abc"),
        ("example.com:443.allowed.com", "443.allowed.com"),
        ("example.com:", ""),
        ("[::1]:0", "0"),
        ("[::1]:65536", "65536"),
        ("[::1]:", ""),
        ("https://example.com", "//example.com"),
    ] {
        let expected = error_of(port.parse::<Port>())?;
        assert_eq!(
            error_of(DomainPattern::new(input))?,
            DomainPatternError::Port(expected),
            "{input}"
        );
    }
    Ok(())
}

#[test]
fn every_pattern_error_has_a_message() {
    let port_error = Port::new(0).err();
    let cases = [
        (DomainPatternError::Empty, "domain pattern must not be empty"),
        (
            DomainPatternError::UnbracketedIpv6("::1".to_string()),
            "IPv6 literal or extra ':' in '::1': IPv6 must be bracketed",
        ),
        (
            DomainPatternError::MalformedBrackets("[::1".to_string()),
            "malformed bracketed address in '[::1'",
        ),
        (
            DomainPatternError::InvalidWildcard("*.com".to_string()),
            "wildcard in '*.com' must be a single leading '*.' with at least two labels after it",
        ),
        (
            DomainPatternError::BareWildcardNotAllowed,
            "bare '*' is only allowed in denied domains",
        ),
        (
            DomainPatternError::SingleLabel("intranet".to_string()),
            "'intranet' must contain a dot or be 'localhost'",
        ),
        (DomainPatternError::Host(HostError::Empty), "host must not be empty"),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
    assert_eq!(
        port_error.map(DomainPatternError::from).map(|error| error.to_string()),
        Some("port '0' is not in 1-65535 without leading zeros".to_string())
    );
}

#[test]
fn a_pattern_serializes_as_its_canonical_string() -> TestResult {
    let parsed = pattern("*.Example.com:443")?;
    assert_eq!(serde_json::to_value(&parsed)?, json!("*.example.com:443"));
    assert_eq!(
        serde_json::from_value::<DomainPattern>(json!("*.EXAMPLE.com:443"))?,
        parsed
    );
    assert_eq!(serde_json::from_str::<DomainPattern>("\"*.EXAMPLE.com:443\"")?, parsed);
    rejects::<DomainPattern>(json!("foo.*.com"))?;
    rejects::<DomainPattern>(json!("*"))?;
    rejects::<DomainPattern>(json!(""))?;
    Ok(())
}

#[test]
fn a_denied_domain_may_be_a_bare_wildcard() -> TestResult {
    let everything = DeniedDomain::new("*")?;
    assert_eq!(everything.to_string(), "*");
    assert_eq!(everything.as_pattern().target(), &PatternTarget::Any);
    assert_eq!(everything.as_pattern().port(), None);

    let ssh = DeniedDomain::new("*:22")?;
    assert_eq!(ssh.to_string(), "*:22");
    assert_eq!(ssh.as_pattern().port(), Some(Port::new(22)?));
    assert_eq!("*:22".parse::<DeniedDomain>()?, ssh);
    assert_eq!(DeniedDomain::try_from("*:22".to_string())?, ssh);
    assert_eq!(String::from(ssh.clone()), "*:22");

    let metadata = DeniedDomain::new("169.254.169.254")?;
    assert_eq!(metadata.to_string(), "169.254.169.254");
    assert_eq!(DeniedDomain::new("2852039166")?, metadata);
    assert_eq!(
        error_of(DeniedDomain::new("*.com"))?,
        DomainPatternError::InvalidWildcard("*.com".to_string())
    );
    assert_eq!(error_of(DeniedDomain::new(""))?, DomainPatternError::Empty);
    Ok(())
}

#[test]
fn a_denied_domain_serializes_as_its_canonical_string() -> TestResult {
    let denied = DeniedDomain::new("*:22")?;
    assert_eq!(serde_json::to_value(&denied)?, json!("*:22"));
    assert_eq!(serde_json::from_value::<DeniedDomain>(json!("*:22"))?, denied);
    assert_eq!(serde_json::from_str::<DeniedDomain>("\"*:22\"")?, denied);
    assert_eq!(
        serde_json::from_value::<DeniedDomain>(json!("*"))?,
        DeniedDomain::new("*")?
    );
    rejects::<DeniedDomain>(json!("*.com"))?;
    rejects::<DeniedDomain>(json!(""))?;
    Ok(())
}
