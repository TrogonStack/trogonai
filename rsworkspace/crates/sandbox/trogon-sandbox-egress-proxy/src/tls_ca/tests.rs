use super::*;
use rsa::pkcs1::EncodeRsaPrivateKey as _;
use rsa::pkcs8::LineEnding;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{ServerName, UnixTime};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use trogon_sandbox_contract::DomainRuleAction;
use x509_parser::extensions::ParsedExtension;
use x509_parser::prelude::{DistributionPointName, FromDer, GeneralName, X509Certificate};

type TestError = Box<dyn std::error::Error>;

fn root_store_for(ca: &SandboxCa) -> Result<rustls::RootCertStore, TestError> {
    let mut root_store = rustls::RootCertStore::empty();
    let mut pem_reader = std::io::Cursor::new(ca.root_cert_pem().into_bytes());
    for cert in rustls_pemfile::certs(&mut pem_reader) {
        root_store.add(cert?)?;
    }
    Ok(root_store)
}

fn verify_chain(
    ca: &SandboxCa,
    certified_key: &rustls::sign::CertifiedKey,
    server_name: &str,
) -> Result<(), rustls::Error> {
    install_default_crypto_provider();
    let general = |error: &dyn std::fmt::Display| rustls::Error::General(error.to_string());
    let roots = root_store_for(ca).map_err(|error| general(&*error))?;
    let verifier = WebPkiServerVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|error| general(&error))?;
    let name = ServerName::try_from(server_name.to_string()).map_err(|error| general(&error))?;
    verifier
        .verify_server_cert(
            &certified_key.cert[0],
            &certified_key.cert[1..],
            &name,
            &[],
            UnixTime::now(),
        )
        .map(|_| ())
}

fn mint_unchecked(ca: &SandboxCa, hostname: &str) -> Result<rustls::sign::CertifiedKey, SandboxCaError> {
    ca.sign_leaf(hostname)
}

fn constrained_ca() -> Result<SandboxCa, SandboxCaError> {
    SandboxCa::generate(PermittedDomains::for_tests(&["api.example.com", "*.github.com"]))
}

#[tokio::test]
async fn leaf_cert_signed_by_ca_validates_against_ca_only_root_store() -> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();
    let ca = SandboxCa::generate(PermittedDomains::for_tests(&["api.example.com"]))?;
    let certified_key = ca.mint_leaf_cert("api.example.com")?;
    let server_config = server_config_for_leaf(certified_key);
    let acceptor = TlsAcceptor::from(server_config);

    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store_for(&ca)?)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut tls = acceptor.accept(stream).await?;
        let mut buf = [0u8; 5];
        tls.read_exact(&mut buf).await?;
        tls.write_all(&buf).await?;
        Ok::<(), std::io::Error>(())
    });

    let tcp = tokio::net::TcpStream::connect(addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from("api.example.com")?;
    let mut tls = connector.connect(server_name, tcp).await?;
    tls.write_all(b"hello").await?;
    let mut echoed = [0u8; 5];
    tls.read_exact(&mut echoed).await?;
    assert_eq!(&echoed, b"hello");

    server_task.await??;
    Ok(())
}

fn leaf_sans(leaf: &rustls::sign::CertifiedKey) -> Result<Vec<String>, TestError> {
    let (_, cert) = X509Certificate::from_der(leaf.cert[0].as_ref())?;
    let Some(san) = cert.subject_alternative_name()? else {
        return Ok(Vec::new());
    };
    san.value
        .general_names
        .iter()
        .map(|name| match name {
            GeneralName::DNSName(dns) => Ok(format!("dns:{dns}")),
            GeneralName::IPAddress(bytes) => match bytes.len() {
                4 => Ok(format!("ip:{}", IpAddr::from(<[u8; 4]>::try_from(*bytes)?))),
                16 => Ok(format!("ip:{}", IpAddr::from(<[u8; 16]>::try_from(*bytes)?))),
                _ => Err("odd IP SAN".into()),
            },
            other => Err(format!("unexpected SAN {other:?}").into()),
        })
        .collect()
}

fn spki_of(leaf: &rustls::sign::CertifiedKey) -> Result<Vec<u8>, TestError> {
    let (_, cert) = X509Certificate::from_der(leaf.cert[0].as_ref())?;
    Ok(cert.public_key().raw.to_vec())
}

#[test]
fn a_leaf_is_cached_per_host_name() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let first = ca.mint_leaf_cert("api.example.com")?;
    let second = ca.mint_leaf_cert("API.example.com")?;
    assert_eq!(first.cert[0].as_ref(), second.cert[0].as_ref());
    let other = ca.mint_leaf_cert("raw.github.com")?;
    assert_ne!(first.cert[0].as_ref(), other.cert[0].as_ref());
    Ok(())
}

#[test]
fn two_cas_never_share_leaves_or_keys() -> Result<(), Box<dyn std::error::Error>> {
    let first = constrained_ca()?;
    let second = constrained_ca()?;
    let a = first.mint_leaf_cert("api.example.com")?;
    let b = second.mint_leaf_cert("api.example.com")?;
    assert_ne!(a.cert[0].as_ref(), b.cert[0].as_ref());
    assert_ne!(spki_of(&a)?, spki_of(&b)?);
    Ok(())
}

#[test]
fn leaves_of_one_ca_share_a_key() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let a = ca.mint_leaf_cert("api.example.com")?;
    let b = ca.mint_leaf_cert("raw.github.com")?;
    assert_eq!(spki_of(&a)?, spki_of(&b)?);
    Ok(())
}

#[test]
fn the_least_recently_used_leaf_is_evicted_first() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = LeafCache::default();
    let ca = constrained_ca()?;
    let leaf = ca.sign_leaf("api.example.com")?;
    for n in 0..LEAF_CACHE_LIMIT {
        cache.insert(&format!("h{n}.example.com"), leaf.clone());
    }
    assert!(cache.get("h0.example.com").is_some());
    cache.insert("extra.example.com", leaf);
    assert!(cache.get("h0.example.com").is_some());
    assert!(cache.get("h1.example.com").is_none());
    assert_eq!(cache.leaves.len(), LEAF_CACHE_LIMIT);
    Ok(())
}

#[test]
fn a_dns_name_gets_a_dns_san() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let leaf = ca.mint_leaf_cert("api.example.com")?;
    assert_eq!(leaf_sans(&leaf)?, vec!["dns:api.example.com"]);
    Ok(())
}

#[test]
fn an_ip_literal_gets_an_ip_san_and_no_dns_san() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let v4 = ca.sign_leaf("127.0.0.1")?;
    assert_eq!(leaf_sans(&v4)?, vec!["ip:127.0.0.1"]);
    let v6 = ca.sign_leaf("::1")?;
    assert_eq!(leaf_sans(&v6)?, vec!["ip:::1"]);
    let bracketed = ca.sign_leaf("[::1]")?;
    assert_eq!(leaf_sans(&bracketed)?, vec!["ip:::1"]);
    Ok(())
}

#[test]
fn root_cert_pem_is_parseable_pem() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let pem = ca.root_cert_pem();
    assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
    Ok(())
}

#[test]
fn root_cert_pem_never_contains_private_key_material() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    assert!(!ca.root_cert_pem().contains("PRIVATE KEY"));
    Ok(())
}

#[test]
fn leaf_for_exact_allowed_name_verifies_against_ca() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let leaf = ca.mint_leaf_cert("api.example.com")?;
    verify_chain(&ca, &leaf, "api.example.com")?;
    Ok(())
}

#[test]
fn leaf_for_wildcard_subdomain_verifies_against_ca() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let leaf = ca.mint_leaf_cert("raw.github.com")?;
    verify_chain(&ca, &leaf, "raw.github.com")?;
    let deep = ca.mint_leaf_cert("a.b.github.com")?;
    verify_chain(&ca, &deep, "a.b.github.com")?;
    Ok(())
}

#[test]
fn leaf_for_disallowed_name_is_refused_at_mint_time() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let err = ca.mint_leaf_cert("evil.com").err().ok_or("expected an error")?;
    assert!(matches!(err, SandboxCaError::HostnameNotPermitted { .. }));
    Ok(())
}

#[test]
fn leaf_for_disallowed_name_fails_chain_verification_on_name_constraints() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    for hostname in ["evil.com", "github.com", "evilgithub.com", "example.com"] {
        let rogue = mint_unchecked(&ca, hostname)?;
        let err = verify_chain(&ca, &rogue, hostname).err().ok_or("expected an error")?;
        assert!(format!("{err:?}").contains("NameConstraint"), "{hostname}: {err:?}");
    }
    Ok(())
}

#[test]
fn wildcard_constraint_does_not_permit_the_bare_apex() -> Result<(), Box<dyn std::error::Error>> {
    let ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.github.com"]))?;
    let apex = mint_unchecked(&ca, "github.com")?;
    assert!(verify_chain(&ca, &apex, "github.com").is_err());
    Ok(())
}

#[test]
fn lease_without_domain_rules_gets_a_ca_that_validates_no_real_host() -> Result<(), Box<dyn std::error::Error>> {
    let permitted = PermittedDomains::from_rules(&[])?;
    assert!(permitted.is_empty());
    let ca = SandboxCa::generate(permitted)?;
    assert!(ca.mint_leaf_cert("api.example.com").is_err());
    let rogue = mint_unchecked(&ca, "api.example.com")?;
    assert!(verify_chain(&ca, &rogue, "api.example.com").is_err());
    Ok(())
}

#[test]
fn the_lease_ca_key_and_its_leaves_are_rsa_2048() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let leaf = ca.mint_leaf_cert("api.example.com")?;
    let (_, root) = X509Certificate::from_der(leaf.cert[1].as_ref())?;
    let spki = root.public_key();
    assert_eq!(spki.algorithm.algorithm.to_id_string(), RSA_ENCRYPTION_OID);
    assert!(spki.subject_public_key.data.len() > 256);
    let (_, leaf_cert) = X509Certificate::from_der(leaf.cert[0].as_ref())?;
    assert_eq!(
        leaf_cert.public_key().algorithm.algorithm.to_id_string(),
        RSA_ENCRYPTION_OID
    );
    assert!(leaf_cert.public_key().subject_public_key.data.len() > 256);
    let signature = leaf_cert.signature_algorithm.algorithm.to_id_string();
    assert_eq!(signature, "1.2.840.113549.1.1.11");
    Ok(())
}

#[test]
fn ca_certificate_carries_critical_name_constraints_for_each_rule() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let pem = ca.root_cert_pem();
    let der = rustls_pemfile::certs(&mut std::io::Cursor::new(pem.into_bytes()))
        .next()
        .ok_or("expected a value")??;
    let (_, cert) = X509Certificate::from_der(der.as_ref())?;
    let extension = cert
        .extensions()
        .iter()
        .find(|ext| ext.oid == x509_parser::oid_registry::OID_X509_EXT_NAME_CONSTRAINTS)
        .ok_or("expected a value")?;
    assert!(extension.critical);
    let ParsedExtension::NameConstraints(constraints) = extension.parsed_extension() else {
        return Err("name constraints did not parse".into());
    };
    let permitted: Vec<String> = constraints
        .permitted_subtrees
        .as_ref()
        .ok_or("expected a value")?
        .iter()
        .map(|subtree| match subtree.base {
            GeneralName::DNSName(name) => Ok(name.to_string()),
            _ => Err("unexpected general name"),
        })
        .collect::<Result<_, _>>()?;
    assert_eq!(permitted, vec!["api.example.com", ".github.com"]);
    Ok(())
}

#[test]
fn covers_is_true_only_for_patterns_inside_the_permitted_set() -> Result<(), Box<dyn std::error::Error>> {
    let permitted = PermittedDomains::for_tests(&["api.example.com", "*.github.com"]);
    let covered =
        |pattern: &str| -> Result<bool, SandboxCaError> { Ok(permitted.covers(&DomainPattern::parse(pattern)?)) };
    assert!(covered("api.example.com")?);
    assert!(covered("raw.github.com")?);
    assert!(covered("*.github.com")?);
    assert!(covered("*.raw.github.com")?);
    assert!(!covered("github.com")?);
    assert!(!covered("*.example.com")?);
    assert!(!covered("other.example.com")?);
    assert!(!covered("evilgithub.com")?);
    assert!(!covered("*.com")?);
    Ok(())
}

#[test]
fn permitted_domains_dedupe_and_normalize_rule_patterns() -> Result<(), Box<dyn std::error::Error>> {
    let rule = |domain: &str| -> Result<DomainRule, TestError> {
        Ok(DomainRule {
            domain: trogon_sandbox_contract::DomainPattern::new(domain)?,
            action: DomainRuleAction::PassThrough,
        })
    };
    let permitted = PermittedDomains::from_rules(&[
        rule("API.Example.com")?,
        rule("api.example.com:443")?,
        rule("api.example.com")?,
        rule("10.0.0.1")?,
    ])?;
    assert_eq!(permitted, PermittedDomains::for_tests(&["api.example.com"]));
    let idna = PermittedDomains::from_rules(&[rule("b\u{fc}cher.example")?])?;
    assert_eq!(idna, PermittedDomains::for_tests(&["xn--bcher-kva.example"]));
    Ok(())
}

#[test]
fn malformed_rule_patterns_never_reach_the_ca() -> Result<(), Box<dyn std::error::Error>> {
    for bad in ["", "*", "*.", "a.*.com", "*.*.com", ".example.com", "a..b"] {
        assert!(
            trogon_sandbox_contract::DomainPattern::new(bad).is_err(),
            "{bad:?} should be rejected"
        );
    }
    Ok(())
}

#[test]
fn leaf_carries_the_authority_key_identifier_of_the_ca() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let leaf = mint_unchecked(&ca, "api.example.com")?;
    let (_, leaf_cert) = X509Certificate::from_der(leaf.cert[0].as_ref())?;
    let (_, ca_cert) = X509Certificate::from_der(leaf.cert[1].as_ref())?;
    let aki = leaf_cert
        .extensions()
        .iter()
        .find_map(|ext| match ext.parsed_extension() {
            ParsedExtension::AuthorityKeyIdentifier(aki) => aki.key_identifier.clone(),
            _ => None,
        })
        .ok_or("expected a value")?;
    let ski = ca_cert
        .extensions()
        .iter()
        .find_map(|ext| match ext.parsed_extension() {
            ParsedExtension::SubjectKeyIdentifier(ski) => Some(ski.0.to_vec()),
            _ => None,
        })
        .ok_or("expected a value")?;
    assert_eq!(aki.0, ski.as_slice());
    Ok(())
}

#[test]
fn validity_is_backdated_and_clamped_to_the_ca() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let leaf = mint_unchecked(&ca, "api.example.com")?;
    let (_, leaf_cert) = X509Certificate::from_der(leaf.cert[0].as_ref())?;
    let (_, ca_cert) = X509Certificate::from_der(leaf.cert[1].as_ref())?;
    let now = ::time::OffsetDateTime::now_utc().unix_timestamp();
    let day = 86_400;

    let leaf_validity = leaf_cert.validity();
    let leaf_span = leaf_validity.not_after.timestamp() - leaf_validity.not_before.timestamp();
    assert_eq!(leaf_span, LEAF_LIFETIME_DAYS * day);
    assert!(leaf_validity.not_before.timestamp() <= now - day + 5);
    assert!(leaf_validity.not_before.timestamp() >= now - day - 60);

    let ca_validity = ca_cert.validity();
    let ca_span = ca_validity.not_after.timestamp() - ca_validity.not_before.timestamp();
    assert_eq!(ca_span, (CA_LIFETIME_DAYS + BACKDATE_DAYS) * day);
    assert!(leaf_validity.not_after.timestamp() <= ca_validity.not_after.timestamp());
    Ok(())
}

#[test]
fn a_generated_pair_validates_and_its_thumbprint_matches() -> Result<(), Box<dyn std::error::Error>> {
    let ca = generate_ca(Some("test ca"), Some(30))?;
    let validated = validate_ca_pair(&ca.cert_pem, &ca.key_pem)?;
    assert_eq!(validated.thumbprint, ca.thumbprint);
    assert_eq!(cert_thumbprint(&ca.cert_pem)?, ca.thumbprint);
    assert_eq!(ca.thumbprint.as_str().len(), 40);
    assert!(
        ca.thumbprint
            .as_str()
            .chars()
            .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
    );
    Ok(())
}

#[test]
fn a_generated_ca_has_an_rsa_2048_key_and_signs_leaves() -> Result<(), Box<dyn std::error::Error>> {
    let generated = generate_ca(None, None)?;
    let der = certificate_der(&generated.cert_pem)?;
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(&der)?;
    assert_eq!(cert.public_key().algorithm.algorithm.to_id_string(), RSA_ENCRYPTION_OID);
    assert!(cert.public_key().subject_public_key.data.len() > 256);
    let ca = SandboxCa::from_pem_pair(
        &generated.cert_pem,
        &generated.key_pem,
        PermittedDomains::for_tests(&["*.example.com"]),
        "ca.pem",
    )?;
    let leaf = ca.mint_leaf_cert("api.example.com")?;
    verify_chain(&ca, &leaf, "api.example.com")?;
    let (_, leaf_cert) = x509_parser::certificate::X509Certificate::from_der(leaf.cert[0].as_ref())?;
    assert_eq!(
        leaf_cert.public_key().algorithm.algorithm.to_id_string(),
        RSA_ENCRYPTION_OID
    );
    Ok(())
}

#[test]
fn a_key_from_another_pair_is_a_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    let first = generate_ca(None, None)?;
    let second = generate_ca(None, None)?;
    assert_eq!(
        validate_ca_pair(&first.cert_pem, &second.key_pem),
        Err(CaPairError::KeyMismatch)
    );
    Ok(())
}

#[test]
fn garbage_is_a_parse_error() -> Result<(), Box<dyn std::error::Error>> {
    let ca = generate_ca(None, None)?;
    assert!(matches!(
        validate_ca_pair("not pem", &ca.key_pem),
        Err(CaPairError::Parse(_))
    ));
    assert!(matches!(
        validate_ca_pair(&ca.cert_pem, "not pem"),
        Err(CaPairError::Parse(_))
    ));
    assert!(matches!(cert_thumbprint("not pem"), Err(CaPairError::Thumbprint(_))));
    Ok(())
}

#[test]
fn a_certificate_not_yet_valid_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.not_before = time::OffsetDateTime::now_utc() + time::Duration::days(3);
    params.not_after = params.not_before + time::Duration::days(30);
    let cert = params.self_signed(&key)?;
    let error = validate_ca_pair(&cert.pem(), &key.serialize_pem())
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, CaPairError::NotYetValid(_)), "{error}");
    Ok(())
}

#[test]
fn leaves_minted_by_a_supplied_ca_chain_to_it() -> Result<(), Box<dyn std::error::Error>> {
    let supplied = generate_ca(None, None)?;
    let ca = SandboxCa::from_pem_pair(
        &supplied.cert_pem,
        &supplied.key_pem,
        PermittedDomains::for_tests(&["*.example.com"]),
        "ca.pem",
    )?;
    assert_eq!(ca.root_cert_pem(), supplied.cert_pem);
    let leaf = ca.mint_leaf_cert("api.example.com")?;
    verify_chain(&ca, &leaf, "api.example.com")?;
    Ok(())
}

#[test]
fn files_that_are_missing_or_not_pem_name_the_field() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let ca = generate_ca(None, None)?;
    let cert = dir.path().join("ca.pem");
    let key = dir.path().join("ca.key");
    std::fs::write(&cert, &ca.cert_pem)?;
    std::fs::write(&key, &ca.key_pem)?;
    let permitted = || PermittedDomains::for_tests(&["a.example.com"]);
    assert!(SandboxCa::from_files(&cert, &key, permitted()).is_ok());

    let missing = dir.path().join("nope.pem");
    let error = SandboxCa::from_files(&missing, &key, permitted())
        .err()
        .ok_or("expected a value")?;
    assert_eq!(
        error.to_string(),
        format!("tlsTerminate.caCertPath: cannot read {} (ENOENT)", missing.display())
    );
    let error = SandboxCa::from_files(&cert, &missing, permitted())
        .err()
        .ok_or("expected a value")?;
    assert_eq!(
        error.to_string(),
        format!("tlsTerminate.caKeyPath: cannot read {} (ENOENT)", missing.display())
    );
    let error = SandboxCa::from_files(&key, &key, permitted())
        .err()
        .ok_or("expected a value")?;
    assert_eq!(
        error.to_string(),
        format!("tlsTerminate.caCertPath: {} is not a PEM CERTIFICATE", key.display())
    );
    let error = SandboxCa::from_files(&cert, &cert, permitted())
        .err()
        .ok_or("expected a value")?;
    assert_eq!(
        error.to_string(),
        format!("tlsTerminate.caKeyPath: {} is not a PEM PRIVATE KEY", cert.display())
    );
    Ok(())
}

/// An RSA CA the way an operator brings one: its certificate, and its key
/// in both the PKCS#8 and the PKCS#1 encoding.
struct RsaCa {
    cert: String,
    pkcs8: String,
    pkcs1: String,
}

fn rsa_ca() -> Result<RsaCa, Box<dyn std::error::Error>> {
    let private = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048)?;
    let pkcs8 = private
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| error.to_string())?
        .to_string();
    let pkcs1 = private
        .to_pkcs1_pem(LineEnding::LF)
        .map_err(|error| error.to_string())?
        .to_string();
    let key = KeyPair::from_pem_and_sign_algo(&pkcs8, &rcgen::PKCS_RSA_SHA256)?;
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "byo test ca");
        dn
    };
    let cert = params.self_signed(&key)?.pem();
    Ok(RsaCa { cert, pkcs8, pkcs1 })
}

#[test]
fn rsa_pairs_in_either_key_encoding_validate_and_sign_leaves() -> Result<(), Box<dyn std::error::Error>> {
    let rsa = rsa_ca()?;
    let pkcs8 = validate_ca_pair(&rsa.cert, &rsa.pkcs8)?;
    let pkcs1 = validate_ca_pair(&rsa.cert, &rsa.pkcs1)?;
    assert_eq!(pkcs8, pkcs1);
    assert_eq!(cert_thumbprint(&rsa.cert)?, pkcs8.thumbprint);
    let ca = SandboxCa::from_pem_pair(
        &rsa.cert,
        &rsa.pkcs1,
        PermittedDomains::for_tests(&["api.example.com"]),
        "ca.pem",
    )?;
    let leaf = ca.mint_leaf_cert("api.example.com")?;
    verify_chain(&ca, &leaf, "api.example.com")?;
    Ok(())
}

fn parsed_crl(ca: &SandboxCa) -> Result<x509_parser::revocation_list::CertificateRevocationList<'_>, TestError> {
    let (rest, crl) = x509_parser::revocation_list::CertificateRevocationList::from_der(ca.crl_der())?;
    assert!(rest.is_empty());
    Ok(crl)
}

fn assert_crl_is_empty_and_signed_by(ca: &SandboxCa) -> Result<(), TestError> {
    let crl = parsed_crl(ca)?;
    let cert_der = ca.ca_cert.der().to_vec();
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(&cert_der)?;
    crl.verify_signature(cert.public_key())?;
    assert_eq!(crl.issuer().as_raw(), cert.subject().as_raw());
    assert_eq!(crl.version().map(|v| v.0), Some(1));
    assert_eq!(crl.iter_revoked_certificates().count(), 0);
    assert_eq!(crl.crl_number().map(|n| n.to_u64_digits()), Some(vec![1]));
    assert_eq!(
        crl.next_update().ok_or("the list has no next update")?.to_datetime(),
        cert.validity().not_after.to_datetime()
    );
    let ski = cert
        .iter_extensions()
        .find_map(|extension| match extension.parsed_extension() {
            ParsedExtension::SubjectKeyIdentifier(id) => Some(id.0),
            _ => None,
        })
        .ok_or("the certificate has no subject key identifier")?;
    let aki = crl
        .extensions()
        .iter()
        .find_map(|extension| match extension.parsed_extension() {
            ParsedExtension::AuthorityKeyIdentifier(aki) => aki.key_identifier.as_ref().map(|id| id.0),
            _ => None,
        })
        .ok_or("the list has no authority key identifier")?;
    assert_eq!(aki, ski);
    Ok(())
}

fn leaf_distribution_points(leaf: &rustls::sign::CertifiedKey) -> Result<Vec<String>, TestError> {
    let (_, cert) = X509Certificate::from_der(leaf.cert[0].as_ref())?;
    Ok(cert
        .iter_extensions()
        .filter_map(|extension| match extension.parsed_extension() {
            ParsedExtension::CRLDistributionPoints(points) => Some(points),
            _ => None,
        })
        .flat_map(|points| points.points.iter())
        .filter_map(|point| match &point.distribution_point {
            Some(DistributionPointName::FullName(names)) => Some(names),
            _ => None,
        })
        .flat_map(|names| names.iter())
        .filter_map(|name| match name {
            GeneralName::URI(uri) => Some(uri.to_string()),
            _ => None,
        })
        .collect())
}

#[test]
fn a_generated_ca_signs_an_empty_revocation_list() -> Result<(), Box<dyn std::error::Error>> {
    assert_crl_is_empty_and_signed_by(&constrained_ca()?)?;
    Ok(())
}

#[test]
fn a_supplied_ca_signs_an_empty_revocation_list() -> Result<(), Box<dyn std::error::Error>> {
    let rsa = rsa_ca()?;
    let ca = SandboxCa::from_pem_pair(
        &rsa.cert,
        &rsa.pkcs8,
        PermittedDomains::for_tests(&["api.example.com"]),
        "ca.pem",
    )?;
    assert_crl_is_empty_and_signed_by(&ca)?;
    Ok(())
}

#[test]
fn a_supplied_ca_without_the_crl_sign_usage_still_gets_a_revocation_list() -> Result<(), Box<dyn std::error::Error>> {
    let key = generate_rsa_key_pair()?;
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "no crl sign");
        dn
    };
    let cert = params.self_signed(&key)?;
    let ca = SandboxCa::from_pem_pair(
        &cert.pem(),
        &key.serialize_pem(),
        PermittedDomains::for_tests(&["api.example.com"]),
        "ca.pem",
    )?;
    assert_crl_is_empty_and_signed_by(&ca)?;
    Ok(())
}

#[test]
fn a_leaf_names_the_crl_url_only_once_one_is_set() -> Result<(), Box<dyn std::error::Error>> {
    let ca = constrained_ca()?;
    let before = ca.mint_leaf_cert("api.example.com")?;
    assert!(leaf_distribution_points(&before)?.is_empty());

    ca.set_crl_url(CrlUrl::loopback(8118));
    let after = ca.mint_leaf_cert("raw.github.com")?;
    assert_eq!(
        leaf_distribution_points(&after)?,
        vec!["http://127.0.0.1:8118/ca.crl".to_string()]
    );
    let cached = ca.mint_leaf_cert("api.example.com")?;
    assert!(leaf_distribution_points(&cached)?.is_empty());
    Ok(())
}

fn ca_cert_and_key(
    algorithm: &'static rcgen::SignatureAlgorithm,
    not_after_days: i64,
) -> Result<(String, String), TestError> {
    let key = KeyPair::generate_for(algorithm)?;
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(60);
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(not_after_days);
    let cert = params.self_signed(&key)?;
    Ok((cert.pem(), key.serialize_pem()))
}

#[test]
fn a_thumbprint_prints_as_its_hex_text() -> Result<(), TestError> {
    let generated = generate_ca(None, None)?;
    assert_eq!(format!("{}", generated.thumbprint), generated.thumbprint.as_str());
    Ok(())
}

#[test]
fn a_key_in_an_encoding_other_than_pkcs8_or_pkcs1_is_a_parse_error() {
    let sec1 = "-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----\n";
    assert!(matches!(key_pair_from_pem(sec1), Err(CaPairError::Parse(_))));
}

#[test]
fn a_key_that_is_neither_rsa_nor_ecdsa_is_refused() -> Result<(), TestError> {
    let (cert, key) = ca_cert_and_key(&rcgen::PKCS_ED25519, 30)?;
    assert_eq!(validate_ca_pair(&cert, &key), Err(CaPairError::KeyType));
    Ok(())
}

#[test]
fn a_supplied_ecdsa_ca_signs_an_empty_revocation_list() -> Result<(), TestError> {
    let (cert, key) = ca_cert_and_key(&rcgen::PKCS_ECDSA_P256_SHA256, 30)?;
    let ca = SandboxCa::from_pem_pair(&cert, &key, PermittedDomains::for_tests(&["api.example.com"]), "ca.pem")?;
    assert!(!ca.crl_der().is_empty());
    Ok(())
}

#[test]
fn an_expired_supplied_ca_still_gets_a_revocation_list_that_expires_in_a_day() -> Result<(), TestError> {
    let (cert, key) = ca_cert_and_key(&rcgen::PKCS_ECDSA_P256_SHA256, -1)?;
    let ca = SandboxCa::from_pem_pair(&cert, &key, PermittedDomains::for_tests(&["api.example.com"]), "ca.pem")?;
    let crl = parsed_crl(&ca)?;
    let next = crl.next_update().ok_or("expected a next update")?.to_datetime();
    assert!(next > time::OffsetDateTime::now_utc());
    Ok(())
}

#[test]
fn a_supplied_pair_that_does_not_parse_names_the_path() {
    let outcome = SandboxCa::from_pem_pair("junk", "junk", PermittedDomains::for_tests(&[]), "given.pem");
    assert!(matches!(
        outcome,
        Err(SandboxCaError::CaPairInvalid { ref path, reason: CaPairError::Parse(_) }) if path == "given.pem"
    ));
}

#[test]
fn a_malformed_domain_pattern_is_refused() {
    assert!(matches!(
        DomainPattern::parse("a..example.com"),
        Err(SandboxCaError::InvalidDomainPattern { .. })
    ));
}

#[test]
fn a_domain_pattern_reads_back_normalized() -> Result<(), TestError> {
    assert_eq!(DomainPattern::parse(" API.Example.com ")?.as_str(), "api.example.com");
    Ok(())
}

#[test]
fn caching_a_leaf_again_keeps_one_entry_and_marks_it_recent() -> Result<(), TestError> {
    let ca = constrained_ca()?;
    let leaf = ca.mint_leaf_cert("api.example.com")?;
    let mut cache = LeafCache::default();
    cache.insert("api.example.com", leaf.clone());
    cache.insert("raw.github.com", leaf.clone());
    cache.insert("api.example.com", leaf);
    assert_eq!(cache.leaves.len(), 2);
    assert_eq!(cache.recency.back().map(String::as_str), Some("api.example.com"));
    Ok(())
}

#[test]
fn a_host_name_outside_ascii_cannot_become_a_leaf() -> Result<(), TestError> {
    let ca = constrained_ca()?.permitting_every_name();
    assert!(matches!(
        ca.mint_leaf_cert("b\u{fc}cher.example"),
        Err(SandboxCaError::LeafCertificateBuild { .. })
    ));
    assert!(matches!(
        sni_to_san("b\u{fc}cher.example"),
        Err(SandboxCaError::InvalidHostname { .. })
    ));
    Ok(())
}

fn unreadable_code(outcome: Result<String, SandboxCaError>) -> Result<String, TestError> {
    match outcome {
        Err(SandboxCaError::CaFileUnreadable { code, .. }) => Ok(code),
        _ => Err("expected an unreadable file".into()),
    }
}

#[test]
fn a_directory_or_a_file_that_is_not_text_is_unreadable_with_its_code() -> Result<(), TestError> {
    let dir = tempfile::tempdir()?;
    let code = unreadable_code(read_pem_file("field", dir.path(), "CERTIFICATE", |_| true))?;
    assert_eq!(code, "EISDIR");

    let binary = dir.path().join("binary.pem");
    std::fs::write(&binary, [0xffu8, 0xfe, 0xfd])?;
    let code = unreadable_code(read_pem_file("field", &binary, "CERTIFICATE", |_| true))?;
    assert!(!code.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_file_the_process_may_not_read_is_unreadable_with_eacces() -> Result<(), TestError> {
    let dir = tempfile::tempdir()?;
    let locked = dir.path().join("locked.pem");
    std::fs::write(&locked, "-----BEGIN CERTIFICATE-----")?;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))?;
    if std::fs::read(&locked).is_ok() {
        return Ok(());
    }
    let code = unreadable_code(read_pem_file("field", &locked, "CERTIFICATE", |_| true))?;
    assert_eq!(code, "EACCES");
    Ok(())
}
