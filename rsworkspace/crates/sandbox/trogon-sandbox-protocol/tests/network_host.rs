use std::net::IpAddr;

use trogon_sandbox_contract::{DeniedDomain, DomainPattern, DomainPatternError, Host, Port};
use trogon_sandbox_protocol::{
    ALLOWED_DOMAIN_ENTRY_MESSAGE, DENIED_DOMAIN_ENTRY_MESSAGE, denied_domain_matches, destination_is_covered_by,
    exact_denied_domain, exact_pattern, pattern_is_covered_by, pattern_matches, pattern_with_port,
    pattern_without_port, target_dns_name, target_ip_literal,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn matches(hostname: &str, port: u16, pattern: &str) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(pattern_matches(&pattern.parse()?, &hostname.parse()?, port))
}

fn covered(pattern: &str, other: &str) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(pattern_is_covered_by(&pattern.parse()?, &other.parse()?))
}

#[test]
fn exact_match_is_case_insensitive() -> TestResult {
    assert!(matches("API.Example.com", 443, "api.example.com")?);
    Ok(())
}

#[test]
fn wildcard_matches_strict_subdomains_only() -> TestResult {
    assert!(matches("a.example.com", 443, "*.example.com")?);
    assert!(matches("a.b.example.com", 443, "*.example.com")?);
    assert!(!matches("example.com", 443, "*.example.com")?);
    assert!(!matches("notexample.com", 443, "*.example.com")?);
    assert!(!matches("evil-example.com", 443, "*.example.com")?);
    Ok(())
}

#[test]
fn wildcard_never_matches_ip_literals() -> TestResult {
    assert!("*.2.3.4".parse::<DomainPattern>().is_err());
    assert!(!matches("[::1]", 443, "*.example.com")?);
    Ok(())
}

#[test]
fn entry_without_port_matches_any_port() -> TestResult {
    assert!(matches("example.com", 22, "example.com")?);
    assert!(matches("a.example.com", 65535, "*.example.com")?);
    Ok(())
}

#[test]
fn entry_with_port_matches_only_that_port() -> TestResult {
    assert!(matches("example.com", 443, "example.com:443")?);
    assert!(!matches("example.com", 80, "example.com:443")?);
    assert!(matches("a.example.com", 8443, "*.example.com:8443")?);
    assert!(!matches("a.example.com", 443, "*.example.com:8443")?);
    Ok(())
}

#[test]
fn bare_star_with_port_in_denied_domains() -> TestResult {
    let denied = DeniedDomain::new("*:22")?;
    assert!(denied_domain_matches(&denied, &"anything.test".parse()?, 22));
    assert!(!denied_domain_matches(&denied, &"anything.test".parse()?, 23));
    assert!(denied_domain_matches(&denied, &"10.0.0.1".parse()?, 22));
    Ok(())
}

#[test]
fn bare_star_is_rejected_in_allowed_patterns() {
    assert!(matches!(
        DomainPattern::new("*:22"),
        Err(DomainPatternError::BareWildcardNotAllowed)
    ));
    assert!(DomainPattern::new("*").is_err());
}

#[test]
fn denied_ip_literal_matches_canonical_spelling() -> TestResult {
    let denied = DeniedDomain::new("169.254.169.254")?;
    assert!(denied_domain_matches(&denied, &"169.254.169.254".parse()?, 80));
    assert!(denied_domain_matches(&denied, &"::ffff:169.254.169.254".parse()?, 80));
    Ok(())
}

#[test]
fn bracketed_ipv6_matches_destination() -> TestResult {
    assert!(matches("::1", 443, "[::1]")?);
    assert!(matches("[0:0:0:0:0:0:0:1]", 443, "[::1]:443")?);
    assert!(!matches("::2", 443, "[::1]")?);
    Ok(())
}

#[test]
fn coverage_follows_names_and_ports() -> TestResult {
    assert!(covered("api.example.com", "*.example.com")?);
    assert!(covered("*.api.example.com", "*.example.com")?);
    assert!(covered("*.example.com", "*.example.com")?);
    assert!(!covered("*.example.com", "api.example.com")?);
    assert!(!covered("example.com", "*.example.com")?);
    assert!(!covered("10.0.0.1", "*.example.com")?);
    assert!(covered("api.example.com:443", "api.example.com")?);
    assert!(!covered("api.example.com", "api.example.com:443")?);
    assert!(covered("api.example.com:443", "api.example.com:443")?);
    assert!(!covered("api.example.com:80", "api.example.com:443")?);
    assert!(!covered("api.example.com", "other.example.com")?);
    let any = DeniedDomain::new("*")?;
    let named: DomainPattern = "a.example.com".parse()?;
    assert!(destination_is_covered_by(
        named.target(),
        named.port(),
        any.target(),
        any.port()
    ));
    assert!(!destination_is_covered_by(
        any.target(),
        any.port(),
        named.target(),
        named.port()
    ));
    Ok(())
}

#[test]
fn ports_can_be_added_and_removed() -> TestResult {
    let pattern: DomainPattern = "*.example.com".parse()?;
    let with = pattern_with_port(&pattern, Port::new(8443)?)?;
    assert_eq!(with.to_string(), "*.example.com:8443");
    assert_eq!(pattern_without_port(&with)?, pattern);
    let named: DomainPattern = "[::1]:443".parse()?;
    assert_eq!(pattern_without_port(&named)?.to_string(), "[::1]");
    assert_eq!(pattern_with_port(&named, Port::new(80)?)?.to_string(), "[::1]:80");
    Ok(())
}

#[test]
fn an_exact_entry_names_one_host_and_port() -> TestResult {
    let host: Host = "Api.Example.com".parse()?;
    let port = Port::new(443)?;
    let pattern = exact_pattern(&host, port)?;
    assert_eq!(pattern.to_string(), "api.example.com:443");
    assert!(pattern_matches(&pattern, &host, 443));
    assert!(!pattern_matches(&pattern, &host, 80));
    let denied = exact_denied_domain(&"::1".parse()?, port)?;
    assert_eq!(denied.to_string(), "[::1]:443");
    Ok(())
}

#[test]
fn a_target_reports_its_address_or_its_dns_name() -> TestResult {
    let ip: DomainPattern = "10.0.0.1".parse()?;
    assert_eq!(target_ip_literal(ip.target()), Some("10.0.0.1".parse::<IpAddr>()?));
    assert_eq!(target_dns_name(ip.target()), None);
    let name: DomainPattern = "example.com".parse()?;
    assert_eq!(target_ip_literal(name.target()), None);
    assert_eq!(target_dns_name(name.target()).as_deref(), Some("example.com"));
    let wildcard: DomainPattern = "*.example.com".parse()?;
    assert_eq!(target_ip_literal(wildcard.target()), None);
    assert_eq!(target_dns_name(wildcard.target()).as_deref(), Some("*.example.com"));
    let any = DeniedDomain::new("*")?;
    assert_eq!(target_ip_literal(any.target()), None);
    assert_eq!(target_dns_name(any.target()), None);
    Ok(())
}

#[test]
fn the_entry_messages_describe_the_grammar() {
    assert!(ALLOWED_DOMAIN_ENTRY_MESSAGE.starts_with("Invalid domain pattern."));
    assert!(DENIED_DOMAIN_ENTRY_MESSAGE.contains("bare \"*\""));
    assert!(!ALLOWED_DOMAIN_ENTRY_MESSAGE.contains("deny-all"));
}
