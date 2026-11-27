use super::*;
use crate::tls_ca::{PermittedDomains, SandboxCa};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn root(name: &str) -> Result<(String, CertificateDer<'static>), Box<dyn std::error::Error>> {
    let mut params = rcgen::CertificateParams::new(vec![name.to_string()])?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok((cert.pem(), cert.der().clone().into_owned()))
}

fn certs_of(bundle: &str) -> Result<Vec<CertificateDer<'static>>, Box<dyn std::error::Error>> {
    Ok(CertificateDer::pem_slice_iter(bundle.as_bytes()).collect::<Result<_, _>>()?)
}

#[test]
fn the_sandbox_ca_comes_first_then_system_roots_then_extras() -> TestResult {
    let ca = SandboxCa::generate(PermittedDomains::for_tests(&["example.com"]))?;
    let (_, system) = root("system.test")?;
    let (extra_pem, extra) = root("extra.test")?;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("extra.pem");
    std::fs::write(&file, extra_pem)?;

    let bundle = assemble(&ca.root_cert_pem(), std::slice::from_ref(&system), [&file]);

    let certs = certs_of(&bundle)?;
    assert_eq!(certs.len(), 3);
    assert_eq!(certs[1], system);
    assert_eq!(certs[2], extra);
    assert!(bundle.starts_with("-----BEGIN CERTIFICATE-----"));
    assert!(bundle.contains(ca.root_cert_pem().trim()));
    Ok(())
}

#[test]
fn only_certificates_are_copied_out_of_an_extra_file() -> TestResult {
    let (cert_pem, cert) = root("extra.test")?;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("combined.pem");
    let key = "-----BEGIN PRIVATE KEY-----\nc2VjcmV0\n-----END PRIVATE KEY-----\n";
    std::fs::write(&file, format!("{key}{cert_pem}"))?;

    let bundle = assemble("", &[], [&file]);

    assert!(!bundle.contains("PRIVATE KEY"));
    assert!(!bundle.contains("c2VjcmV0"));
    assert_eq!(certs_of(&bundle)?, vec![cert]);
    Ok(())
}

#[test]
fn a_missing_or_certificate_free_extra_file_is_skipped() -> TestResult {
    let dir = tempfile::tempdir()?;
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "not a certificate\n")?;
    let missing = dir.path().join("missing.pem");
    let (_, system) = root("system.test")?;

    let bundle = assemble("", std::slice::from_ref(&system), [&missing, &empty]);

    assert_eq!(certs_of(&bundle)?, vec![system]);
    Ok(())
}

#[test]
fn a_certificate_listed_twice_appears_once() -> TestResult {
    let (_, system) = root("system.test")?;
    let bundle = assemble("", &[system.clone(), system.clone()], []);
    assert_eq!(certs_of(&bundle)?.len(), 1);
    Ok(())
}

#[test]
fn skipped_extra_files_are_reported_when_logging_is_enabled() -> TestResult {
    let dir = tempfile::tempdir()?;
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "not a certificate\n")?;
    let missing = dir.path().join("missing.pem");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(std::io::sink)
        .finish();

    let bundle = tracing::subscriber::with_default(subscriber, || assemble("", &[], [&missing, &empty]));

    assert!(bundle.trim().is_empty());
    Ok(())
}

#[test]
fn the_workload_bundle_leads_with_the_sandbox_ca_and_carries_the_extra_files() -> TestResult {
    let ca = SandboxCa::generate(PermittedDomains::for_tests(&["example.com"]))?;
    let (extra_pem, extra) = root("extra.test")?;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("extra.pem");
    std::fs::write(&file, extra_pem)?;

    let bundle = workload_trust_bundle(&ca.root_cert_pem(), &[file]);

    assert!(bundle.starts_with(ca.root_cert_pem().trim()));
    assert!(certs_of(&bundle)?.contains(&extra));
    Ok(())
}
