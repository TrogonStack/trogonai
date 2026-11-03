use super::*;

fn client_cert() -> ClientCertificate {
    ClientCertificate {
        cert: Location::Path("/c.pem".into()),
        key: Location::Path("/k.pem".into()),
    }
}

#[test]
fn default_is_tls_against_the_system_roots() {
    assert_eq!(
        NatsTls::default(),
        NatsTls::Enabled {
            ca: None,
            client_cert: None
        }
    );
}

#[test]
fn description_names_the_transport() {
    let private_ca = NatsTls::Enabled {
        ca: Some(Location::Path("/ca.pem".into())),
        client_cert: None,
    };
    let mutual = NatsTls::Enabled {
        ca: None,
        client_cert: Some(client_cert()),
    };

    assert_eq!(NatsTls::Disabled.description(), "disabled");
    assert_eq!(NatsTls::default().description(), "TLS");
    assert_eq!(private_ca.description(), "TLS");
    assert_eq!(mutual.description(), "mutual TLS");
}

#[test]
fn incomplete_client_certificate_names_the_missing_setting() {
    assert_eq!(IncompleteClientCertificate::MissingCert.missing_setting(), "NATS_CERT");
    assert_eq!(IncompleteClientCertificate::MissingKey.missing_setting(), "NATS_KEY");
    assert_eq!(IncompleteClientCertificate::MissingCert.present_setting(), "NATS_KEY");
    assert_eq!(IncompleteClientCertificate::MissingKey.present_setting(), "NATS_CERT");
}
