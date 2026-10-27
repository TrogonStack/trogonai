use super::*;

fn location(raw: &str) -> Option<Location> {
    Location::parse(raw)
}

#[test]
fn default_is_not_configured() {
    assert!(!NatsTls::default().is_configured());
}

#[test]
fn client_certificate_requires_cert_and_key() {
    let cert_only = NatsTls::new(location("/c.pem"), None, None);
    let key_only = NatsTls::new(None, location("/k.pem"), None);

    assert!(cert_only.client_certificate().is_none());
    assert!(key_only.client_certificate().is_none());
    assert!(!cert_only.is_configured());
    assert!(!key_only.is_configured());
}

#[test]
fn cert_and_key_form_a_client_certificate() {
    let tls = NatsTls::new(location("/c.pem"), location("file:///k.pem"), None);
    let client = tls.client_certificate().unwrap();

    assert_eq!(client.cert, Location::Path("/c.pem".into()));
    assert_eq!(client.key, Location::Path("/k.pem".into()));
    assert!(tls.is_configured());
}

#[test]
fn ca_alone_is_configured() {
    let tls = NatsTls::new(None, None, location("/ca.pem"));

    assert!(tls.is_configured());
    assert!(tls.client_certificate().is_none());
    assert_eq!(tls.root_ca(), Some(&Location::Path("/ca.pem".into())));
}
