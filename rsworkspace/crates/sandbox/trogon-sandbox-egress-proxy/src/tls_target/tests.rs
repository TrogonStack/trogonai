use super::*;

#[test]
fn an_ipv6_host_is_bracketed_in_the_authority() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(TlsTarget::new("::1", 443).authority(), "[::1]");
    assert_eq!(TlsTarget::new("::1", 8443).authority(), "[::1]:8443");
    assert_eq!(TlsTarget::new("Example.COM", 8443).authority(), "example.com:8443");
    assert_eq!(TlsTarget::new("example.com", 8443).port(), 8443);
    Ok(())
}

#[test]
fn a_host_with_stray_brackets_names_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let target = TlsTarget::new("example.com", 443);
    assert!(!target.names("exa]mple.com"));
    assert!(!target.names("exa[mple.com:443"));
    assert!(target.names("example.com"));
    Ok(())
}
