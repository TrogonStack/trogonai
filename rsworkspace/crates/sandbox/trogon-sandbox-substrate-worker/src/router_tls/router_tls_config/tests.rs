use std::path::{Path, PathBuf};

use rcgen::{CertificateParams, KeyPair};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SERVER_NAME: &str = "atenet-router.ate-system.svc";

struct Material {
    directory: tempfile::TempDir,
    cert_pem: String,
    key_pem: String,
}

impl Material {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let key = KeyPair::generate()?;
        let cert = CertificateParams::new(vec![SERVER_NAME.to_string()])?.self_signed(&key)?;
        Ok(Self {
            directory: tempfile::tempdir()?,
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
        })
    }

    fn write(&self, name: &str, contents: &str) -> Result<PathBuf, std::io::Error> {
        let path = self.directory.path().join(name);
        std::fs::write(&path, contents)?;
        Ok(path)
    }

    fn bundle(&self) -> String {
        format!("{}{}", self.key_pem, self.cert_pem)
    }
}

fn failure(ca: &Path, cert: &Path, key: &Path) -> Result<RouterTlsError, Box<dyn std::error::Error>> {
    let config = RouterTlsConfig::new(ca, cert, key, SERVER_NAME)?;
    config.load().err().ok_or_else(|| "loading must fail".into())
}

#[test]
fn every_path_and_a_valid_server_name_are_required() {
    for (ca, cert, key, name) in [
        ("", "client.pem", "client.pem", SERVER_NAME),
        ("ca.pem", "", "client.pem", SERVER_NAME),
        ("ca.pem", "client.pem", "", SERVER_NAME),
    ] {
        assert!(matches!(
            RouterTlsConfig::new(ca, cert, key, name),
            Err(RouterTlsError::MissingPath)
        ));
    }
    for name in ["", "https://router:8444"] {
        assert!(matches!(
            RouterTlsConfig::new("ca.pem", "client.pem", "client.pem", name),
            Err(RouterTlsError::InvalidServerName)
        ));
    }
}

#[test]
fn a_combined_identity_file_loads() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &material.cert_pem)?;
    let bundle = material.write("bundle.pem", &material.bundle())?;
    RouterTlsConfig::new(&ca, &bundle, &bundle, SERVER_NAME)?
        .load()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[test]
fn a_missing_file_is_reported_as_unopenable() -> TestResult {
    let material = Material::new()?;
    let missing = material.directory.path().join("missing.pem");
    assert!(matches!(
        failure(&missing, &missing, &missing)?,
        RouterTlsError::Open { .. }
    ));
    Ok(())
}

#[test]
fn a_directory_in_place_of_a_file_is_reported_as_unreadable() -> TestResult {
    let material = Material::new()?;
    let directory = material.directory.path();
    assert!(matches!(
        failure(directory, directory, directory)?,
        RouterTlsError::Read { .. }
    ));
    Ok(())
}

#[test]
fn an_oversized_file_is_refused() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &"A".repeat(1024 * 1024 + 1))?;
    assert!(matches!(failure(&ca, &ca, &ca)?, RouterTlsError::PemTooLarge { .. }));
    Ok(())
}

#[test]
fn a_certificate_block_that_does_not_decode_is_invalid_pem() -> TestResult {
    let material = Material::new()?;
    let ca = material.write(
        "ca.pem",
        "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n",
    )?;
    assert!(matches!(failure(&ca, &ca, &ca)?, RouterTlsError::InvalidPem { .. }));
    Ok(())
}

#[test]
fn a_file_without_certificates_is_refused() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", "nothing to see here")?;
    assert!(matches!(failure(&ca, &ca, &ca)?, RouterTlsError::NoCertificates { .. }));
    Ok(())
}

#[test]
fn a_certificate_block_that_is_not_a_certificate_is_refused_as_a_trust_anchor() -> TestResult {
    let material = Material::new()?;
    let ca = material.write(
        "ca.pem",
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )?;
    let bundle = material.write("bundle.pem", &material.bundle())?;
    assert!(matches!(
        failure(&ca, &bundle, &bundle)?,
        RouterTlsError::InvalidCaCertificate
    ));
    Ok(())
}

#[test]
fn a_missing_client_certificate_is_reported() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &material.cert_pem)?;
    let missing = material.directory.path().join("missing.pem");
    assert!(matches!(failure(&ca, &missing, &missing)?, RouterTlsError::Open { .. }));
    Ok(())
}

#[test]
fn a_missing_separate_key_file_is_reported() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &material.cert_pem)?;
    let cert = material.write("cert.pem", &material.cert_pem)?;
    let missing = material.directory.path().join("missing.pem");
    assert!(matches!(failure(&ca, &cert, &missing)?, RouterTlsError::Open { .. }));
    Ok(())
}

#[test]
fn an_identity_without_a_private_key_is_refused() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &material.cert_pem)?;
    let cert = material.write("cert.pem", &material.cert_pem)?;
    assert!(matches!(failure(&ca, &cert, &cert)?, RouterTlsError::MissingPrivateKey));
    Ok(())
}

#[test]
fn a_private_key_block_that_does_not_decode_is_refused() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &material.cert_pem)?;
    let cert = material.write("cert.pem", &material.cert_pem)?;
    let key = material.write(
        "key.pem",
        "-----BEGIN PRIVATE KEY-----\n!!!!\n-----END PRIVATE KEY-----\n",
    )?;
    let error = failure(&ca, &cert, &key)?;
    assert!(matches!(error, RouterTlsError::InvalidKeyPem), "{error:?}");
    Ok(())
}

#[test]
fn a_key_that_does_not_match_the_certificate_is_refused() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &material.cert_pem)?;
    let cert = material.write("cert.pem", &material.cert_pem)?;
    let other_key = material.write("other-key.pem", &KeyPair::generate()?.serialize_pem())?;
    assert!(matches!(
        failure(&ca, &cert, &other_key)?,
        RouterTlsError::InvalidClientIdentity
    ));
    Ok(())
}

#[tokio::test]
async fn connecting_to_a_closed_port_is_reported() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &material.cert_pem)?;
    let bundle = material.write("bundle.pem", &material.bundle())?;
    let config = RouterTlsConfig::new(&ca, &bundle, &bundle, SERVER_NAME)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    drop(listener);
    let error = config.connect(&address).await.err().ok_or("nothing listens there")?;
    assert!(matches!(error, RouterTlsError::Connect(_)));
    Ok(())
}

#[tokio::test]
async fn a_peer_that_does_not_speak_tls_fails_the_handshake() -> TestResult {
    let material = Material::new()?;
    let ca = material.write("ca.pem", &material.cert_pem)?;
    let bundle = material.write("bundle.pem", &material.bundle())?;
    let config = RouterTlsConfig::new(&ca, &bundle, &bundle, SERVER_NAME)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
        }
    });
    let error = config.connect(&address).await.err().ok_or("not a TLS peer")?;
    assert!(matches!(error, RouterTlsError::Handshake(_)));
    Ok(())
}

#[tokio::test]
async fn unreadable_material_fails_before_any_connection() -> TestResult {
    let material = Material::new()?;
    let missing = material.directory.path().join("missing.pem");
    let config = RouterTlsConfig::new(&missing, &missing, &missing, SERVER_NAME)?;
    let error = config.connect("127.0.0.1:1").await.err().ok_or("missing files")?;
    assert!(matches!(error, RouterTlsError::Open { .. }));
    Ok(())
}
