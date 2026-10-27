use super::*;

fn location(raw: &str) -> Option<Location> {
    Location::parse(raw)
}

#[test]
fn default_is_not_configured() {
    let tls = NatsTls::default();

    assert!(!tls.is_configured());
    assert_eq!(tls.client_certificate(), Ok(None));
}

#[test]
fn lone_cert_or_key_is_reported_as_incomplete() {
    let cert_only = NatsTls::new(location("/c.pem"), None, None);
    let key_only = NatsTls::new(None, location("/k.pem"), None);

    assert_eq!(
        cert_only.client_certificate(),
        Err(IncompleteClientCertificate::MissingKey)
    );
    assert_eq!(
        key_only.client_certificate(),
        Err(IncompleteClientCertificate::MissingCert)
    );
    assert!(cert_only.is_configured());
    assert!(key_only.is_configured());
}

#[test]
fn incomplete_client_certificate_names_the_missing_setting() {
    assert_eq!(IncompleteClientCertificate::MissingCert.missing_setting(), "NATS_CERT");
    assert_eq!(IncompleteClientCertificate::MissingKey.missing_setting(), "NATS_KEY");
    assert_eq!(IncompleteClientCertificate::MissingCert.present_setting(), "NATS_KEY");
    assert_eq!(IncompleteClientCertificate::MissingKey.present_setting(), "NATS_CERT");
}

#[test]
fn cert_and_key_form_a_client_certificate() {
    let tls = NatsTls::new(location("/c.pem"), location("file:///k.pem"), None);
    let client = tls.client_certificate().unwrap().unwrap();

    assert_eq!(client.cert, Location::Path("/c.pem".into()));
    assert_eq!(client.key, Location::Path("/k.pem".into()));
    assert!(tls.is_configured());
}

#[test]
fn ca_alone_is_configured() {
    let tls = NatsTls::new(None, None, location("/ca.pem"));

    assert!(tls.is_configured());
    assert_eq!(tls.client_certificate(), Ok(None));
    assert_eq!(tls.root_ca(), Some(&Location::Path("/ca.pem".into())));
}
