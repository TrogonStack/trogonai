//! Per-lease sandbox root CA: generated in memory when the proxy starts, with
//! an RSA-2048 key and a name constraint limited to the lease's domain
//! rules, used to mint fresh leaf certificates for TLS-terminating a matched
//! domain rule. The CA private key is never serialized; only the certificate
//! leaves the process.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};

use rcgen::PublicKeyData;
use rcgen::SigningKey;
use rcgen::string::Ia5String;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CrlDistributionPoint, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints, SanType,
};
use rsa::pkcs8::EncodePrivateKey;
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::pem::PemObject;
use sha1::Digest;
use trogon_sandbox_contract::DomainRule;
use x509_parser::prelude::FromDer;

mod constants;
use constants::{
    BACKDATE_DAYS, CA_LIFETIME_DAYS, DEFAULT_GENERATED_CA_CN, DEFAULT_GENERATED_CA_VALIDITY_DAYS, DER_BIT_STRING,
    DER_EXTENSIONS, DER_GENERALIZED_TIME, DER_INTEGER, DER_KEY_IDENTIFIER, DER_NULL, DER_OCTET_STRING, DER_SEQUENCE,
    DER_UTC_TIME, EC_PUBLIC_KEY_OID, LEAF_LIFETIME_DAYS, OID_AUTHORITY_KEY_IDENTIFIER, OID_CRL_NUMBER,
    OID_ECDSA_WITH_SHA256, OID_SHA256_WITH_RSA, RSA_ENCRYPTION_OID, RSA_KEY_BITS, UNREACHABLE_SUBTREE,
    UTC_TIME_LAST_YEAR,
};
pub use constants::{CRL_PATH, LEAF_CACHE_LIMIT};

#[derive(Debug, thiserror::Error)]
pub enum SandboxCaError {
    #[error("failed to generate CA key pair: {0}")]
    KeyGeneration(rcgen::Error),
    #[error("failed to generate RSA CA key: {0}")]
    RsaKeyGeneration(String),
    #[error("failed to build CA certificate: {0}")]
    CertificateBuild(rcgen::Error),
    #[error("failed to build leaf certificate for {hostname}: {source}")]
    LeafCertificateBuild { hostname: String, source: rcgen::Error },
    #[error("hostname {hostname} is not a valid DNS SAN entry")]
    InvalidHostname { hostname: String },
    #[error("domain pattern {pattern:?} cannot be expressed as a name constraint")]
    InvalidDomainPattern { pattern: String },
    #[error("hostname {hostname} is outside the CA's permitted domains")]
    HostnameNotPermitted { hostname: String },
    #[error("failed to build the empty revocation list: {0}")]
    CrlBuild(String),
    #[error("failed to build rustls certified key: {0}")]
    CertifiedKey(rustls::Error),
    #[error("tlsTerminate: caCertPath and caKeyPath must be provided together")]
    CaPathsNotPaired,
    #[error("{field}: cannot read {path} ({code})")]
    CaFileUnreadable {
        field: &'static str,
        path: String,
        code: String,
    },
    #[error("{field}: {path} is not a PEM {label}")]
    CaFileNotPem {
        field: &'static str,
        path: String,
        label: &'static str,
    },
    #[error("tlsTerminate: CA at {path}: {reason}")]
    CaPairInvalid { path: String, reason: CaPairError },
}

/// Why a certificate and private key cannot serve as a signing CA.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum CaPairError {
    #[error("parse: {0}")]
    Parse(String),
    #[error("key is not RSA")]
    KeyType,
    #[error("notBefore {0} is in the future (clock skew?)")]
    NotYetValid(String),
    #[error("key does not match certificate public key")]
    KeyMismatch,
    #[error("thumbprint: {0}")]
    Thumbprint(String),
}

/// The SHA-1 fingerprint of a CA certificate as uppercase hex without
/// separators, the form `openssl x509 -fingerprint` prints minus the colons.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CaThumbprint(String);

impl CaThumbprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CaThumbprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A CA certificate and private key that passed [`validate_ca_pair`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ValidatedCa {
    pub thumbprint: CaThumbprint,
    pub not_after: time::OffsetDateTime,
}

/// A freshly generated CA, both halves PEM encoded.
pub struct GeneratedCa {
    pub cert_pem: String,
    pub key_pem: String,
    pub thumbprint: CaThumbprint,
}

/// Generates a self-signed CA that can sign leaves, with an RSA-2048 key. The signing backend cannot generate RSA keys, so
/// the key comes from the `rsa` crate and is handed over as PKCS#8.
pub fn generate_ca(common_name: Option<&str>, validity_days: Option<i64>) -> Result<GeneratedCa, SandboxCaError> {
    let now = time::OffsetDateTime::now_utc();
    let key = generate_rsa_key_pair()?;
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, common_name.unwrap_or(DEFAULT_GENERATED_CA_CN));
        dn
    };
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.use_authority_key_identifier_extension = true;
    params.not_before = now - time::Duration::days(BACKDATE_DAYS);
    params.not_after = now + time::Duration::days(validity_days.unwrap_or(DEFAULT_GENERATED_CA_VALIDITY_DAYS));
    let cert = params.self_signed(&key).map_err(SandboxCaError::CertificateBuild)?;
    let thumbprint = fingerprint(cert.der());
    Ok(GeneratedCa {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
        thumbprint,
    })
}

fn generate_rsa_key_pair() -> Result<KeyPair, SandboxCaError> {
    let private = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, RSA_KEY_BITS)
        .map_err(|error| SandboxCaError::RsaKeyGeneration(error.to_string()))?;
    let pkcs8 = private
        .to_pkcs8_der()
        .map_err(|error| SandboxCaError::RsaKeyGeneration(error.to_string()))?;
    KeyPair::from_pkcs8_der_and_sign_algo(
        &rustls::pki_types::PrivatePkcs8KeyDer::from(pkcs8.as_bytes().to_vec()),
        &rcgen::PKCS_RSA_SHA256,
    )
    .map_err(SandboxCaError::KeyGeneration)
}

fn fingerprint(der: &[u8]) -> CaThumbprint {
    CaThumbprint(hex::encode_upper(sha1::Sha1::digest(der)))
}

fn certificate_der(pem: &str) -> Result<rustls::pki_types::CertificateDer<'static>, CaPairError> {
    rustls::pki_types::CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|error| CaPairError::Parse(error.to_string()))
}

/// The SHA-1 thumbprint of the first certificate in `cert_pem`.
pub fn cert_thumbprint(cert_pem: &str) -> Result<CaThumbprint, CaPairError> {
    certificate_der(cert_pem)
        .map(|der| fingerprint(&der))
        .map_err(|error| CaPairError::Thumbprint(error.to_string().trim_start_matches("parse: ").to_string()))
}

fn der_length(len: usize) -> Vec<u8> {
    if len < 0x80 {
        vec![len as u8]
    } else {
        let bytes: Vec<u8> = len.to_be_bytes().into_iter().skip_while(|byte| *byte == 0).collect();
        let mut out = vec![0x80 | bytes.len() as u8];
        out.extend(bytes);
        out
    }
}

fn der_tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend(der_length(body.len()));
    out.extend_from_slice(body);
    out
}

fn pkcs1_to_pkcs8(pkcs1: &[u8]) -> Vec<u8> {
    let algorithm = der_tlv(
        0x30,
        &[
            &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01][..],
            &[0x05, 0x00][..],
        ]
        .concat(),
    );
    let body = [&[0x02, 0x01, 0x00][..], &algorithm, &der_tlv(0x04, pkcs1)].concat();
    der_tlv(0x30, &body)
}

fn key_pair_from_pem(key_pem: &str) -> Result<KeyPair, CaPairError> {
    let parse = |error: &dyn std::fmt::Display| CaPairError::Parse(error.to_string());
    let der = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|e| parse(&e))?;
    match der {
        PrivateKeyDer::Pkcs8(pkcs8) => KeyPair::try_from(&pkcs8).map_err(|e| parse(&e)),
        PrivateKeyDer::Pkcs1(pkcs1) => {
            KeyPair::try_from(pkcs1_to_pkcs8(pkcs1.secret_pkcs1_der())).map_err(|e| parse(&e))
        }
        _ => Err(CaPairError::Parse("unsupported private key encoding".to_string())),
    }
}

fn parsed_public_key_algorithm(spki_der: &[u8]) -> Result<String, CaPairError> {
    x509_parser::x509::SubjectPublicKeyInfo::from_der(spki_der)
        .map(|(_, spki)| spki.algorithm.algorithm.to_id_string())
        .map_err(|error| CaPairError::Parse(error.to_string()))
}

fn validate_loaded(
    cert_der: &rustls::pki_types::CertificateDer<'_>,
    key: &KeyPair,
) -> Result<ValidatedCa, CaPairError> {
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(cert_der)
        .map_err(|error| CaPairError::Parse(error.to_string()))?;
    let algorithm = parsed_public_key_algorithm(&key.subject_public_key_info())?;
    if algorithm != RSA_ENCRYPTION_OID && algorithm != EC_PUBLIC_KEY_OID {
        return Err(CaPairError::KeyType);
    }
    let validity = cert.validity();
    let not_before = validity.not_before.to_datetime();
    if not_before > time::OffsetDateTime::now_utc() {
        return Err(CaPairError::NotYetValid(
            not_before
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| not_before.to_string()),
        ));
    }
    if cert.public_key().raw != key.subject_public_key_info().as_slice() {
        return Err(CaPairError::KeyMismatch);
    }
    Ok(ValidatedCa {
        thumbprint: fingerprint(cert_der),
        not_after: validity.not_after.to_datetime(),
    })
}

/// Checks that `key_pem` is the private half of `cert_pem` and that the
/// certificate is already valid, as a client would require. ECDSA P-256 keys are
/// accepted besides RSA.
pub fn validate_ca_pair(cert_pem: &str, key_pem: &str) -> Result<ValidatedCa, CaPairError> {
    let cert = certificate_der(cert_pem)?;
    let key = key_pair_from_pem(key_pem)?;
    validate_loaded(&cert, &key)
}

fn read_pem_file(
    field: &'static str,
    path: &std::path::Path,
    label: &'static str,
    marker: impl Fn(&str) -> bool,
) -> Result<String, SandboxCaError> {
    let text = std::fs::read_to_string(path).map_err(|error| SandboxCaError::CaFileUnreadable {
        field,
        path: path.display().to_string(),
        code: match error.kind() {
            std::io::ErrorKind::NotFound => "ENOENT".to_string(),
            std::io::ErrorKind::PermissionDenied => "EACCES".to_string(),
            std::io::ErrorKind::IsADirectory => "EISDIR".to_string(),
            _ => error.to_string(),
        },
    })?;
    if marker(&text) {
        Ok(text)
    } else {
        Err(SandboxCaError::CaFileNotPem {
            field,
            path: path.display().to_string(),
            label,
        })
    }
}

/// A lease domain rule pattern: an exact host name, or a single leading `*.`
/// wildcard matching strict subdomains (the same shape
/// [`trogon_sandbox_contract::DomainPattern`] accepts).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DomainPattern(String);

impl DomainPattern {
    pub fn parse(pattern: &str) -> Result<Self, SandboxCaError> {
        let normalized = pattern.trim().to_ascii_lowercase();
        let name = normalized.strip_prefix("*.").unwrap_or(&normalized);
        let well_formed = !name.is_empty()
            && name.is_ascii()
            && !name.starts_with('.')
            && !name.ends_with('.')
            && !name.contains("..")
            && !name.contains('*');
        if well_formed {
            Ok(Self(normalized))
        } else {
            Err(SandboxCaError::InvalidDomainPattern {
                pattern: pattern.to_string(),
            })
        }
    }

    pub fn from_rule(pattern: &trogon_sandbox_contract::DomainPattern) -> Option<Self> {
        trogon_sandbox_protocol::target_dns_name(pattern.target()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, hostname: &str) -> bool {
        let hostname = hostname.to_ascii_lowercase();
        match self.0.strip_prefix("*.") {
            Some(suffix) => hostname
                .strip_suffix(suffix)
                .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.')),
            None => self.0 == hostname,
        }
    }

    fn covers(&self, other: &DomainPattern) -> bool {
        match (self.0.strip_prefix("*."), other.0.strip_prefix("*.")) {
            (Some(own), Some(theirs)) => theirs == own || theirs.ends_with(&format!(".{own}")),
            (Some(_), None) => self.matches(&other.0),
            (None, Some(_)) => false,
            (None, None) => self.0 == other.0,
        }
    }

    /// RFC 5280 dNSName constraints: `example.com` permits that name and every
    /// subdomain, while `.example.com` permits subdomains only, which is the
    /// meaning of a `*.example.com` rule.
    fn permitted_subtree(&self) -> GeneralSubtree {
        let constraint = self.0.strip_prefix('*').unwrap_or(&self.0);
        GeneralSubtree::DnsName(constraint.to_string())
    }
}

/// The set of names a lease's CA may vouch for. An empty set is not "no
/// restriction": it constrains the CA to the reserved `.invalid` TLD, so a CA
/// created for a lease without domain rules cannot validate any real host.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PermittedDomains(Vec<DomainPattern>);

impl PermittedDomains {
    pub fn from_rules(rules: &[DomainRule]) -> Result<Self, SandboxCaError> {
        let mut patterns: Vec<DomainPattern> = Vec::new();
        for rule in rules {
            if let Some(pattern) = DomainPattern::from_rule(&rule.domain)
                && !patterns.contains(&pattern)
            {
                patterns.push(pattern);
            }
        }
        Ok(Self(patterns))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn permits(&self, hostname: &str) -> bool {
        self.0.iter().any(|pattern| pattern.matches(hostname))
    }

    /// Whether every name `pattern` can match is one this CA may vouch for.
    pub fn covers(&self, pattern: &DomainPattern) -> bool {
        self.0.iter().any(|permitted| permitted.covers(pattern))
    }

    fn name_constraints(&self) -> NameConstraints {
        let permitted_subtrees = if self.0.is_empty() {
            vec![GeneralSubtree::DnsName(UNREACHABLE_SUBTREE.to_string())]
        } else {
            self.0.iter().map(DomainPattern::permitted_subtree).collect()
        };
        NameConstraints {
            permitted_subtrees,
            excluded_subtrees: Vec::new(),
        }
    }
}

/// Installs `aws-lc-rs` as the process-wide rustls `CryptoProvider`, if one is not
/// already installed. Both `trogon-sandbox-egress-proxyd`'s startup and this crate's
/// tests need exactly one provider installed before any TLS handshake or
/// `ServerConfig`/`ClientConfig` builder runs; calling this more than once
/// (e.g. across multiple tests in the same process) is safe.
pub fn install_default_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// The most recently used leaf certificates of one CA, least recently used
/// first out once [`LEAF_CACHE_LIMIT`] is reached.
#[derive(Default)]
struct LeafCache {
    leaves: HashMap<String, rustls::sign::CertifiedKey>,
    recency: VecDeque<String>,
}

impl LeafCache {
    fn get(&mut self, hostname: &str) -> Option<rustls::sign::CertifiedKey> {
        let leaf = self.leaves.get(hostname)?.clone();
        self.touch(hostname);
        Some(leaf)
    }

    fn insert(&mut self, hostname: &str, leaf: rustls::sign::CertifiedKey) {
        if self.leaves.insert(hostname.to_string(), leaf).is_some() {
            self.touch(hostname);
            return;
        }
        self.recency.push_back(hostname.to_string());
        while self.recency.len() > LEAF_CACHE_LIMIT {
            if let Some(oldest) = self.recency.pop_front() {
                self.leaves.remove(&oldest);
            }
        }
    }

    fn touch(&mut self, hostname: &str) {
        if let Some(at) = self.recency.iter().position(|cached| cached == hostname) {
            self.recency.remove(at);
        }
        self.recency.push_back(hostname.to_string());
    }
}

enum CaCertificate {
    Generated(Certificate),
    Supplied {
        der: rustls::pki_types::CertificateDer<'static>,
        pem: String,
    },
}

impl CaCertificate {
    fn der(&self) -> &[u8] {
        match self {
            Self::Generated(cert) => cert.der(),
            Self::Supplied { der, .. } => der,
        }
    }

    fn pem(&self) -> String {
        match self {
            Self::Generated(cert) => cert.pem(),
            Self::Supplied { pem, .. } => pem.clone(),
        }
    }
}

/// A self-signed, name-constrained root CA created for one lease (the proxy
/// runs one process per lease), used to mint leaf certificates on demand. All
/// leaves share one key, and the latest [`LEAF_CACHE_LIMIT`] are kept per
/// host name. The signing keys live only in this value's memory.
pub struct SandboxCa {
    ca_cert: CaCertificate,
    issuer: Issuer<'static, KeyPair>,
    leaf_key: OnceLock<KeyPair>,
    ca_not_after: time::OffsetDateTime,
    leaves: Mutex<LeafCache>,
    permitted: PermittedDomains,
    any_name: bool,
    crl_der: Vec<u8>,
    crl_url: OnceLock<CrlUrl>,
}

impl SandboxCa {
    pub fn generate(permitted: PermittedDomains) -> Result<Self, SandboxCaError> {
        let now = time::OffsetDateTime::now_utc();
        let ca_key = generate_rsa_key_pair()?;

        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.name_constraints = Some(permitted.name_constraints());
        params.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "Sandbox Egress Proxy Root CA");
            dn
        };
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.not_before = now - time::Duration::days(BACKDATE_DAYS);
        params.not_after = now + time::Duration::days(CA_LIFETIME_DAYS);

        let ca_cert = params.self_signed(&ca_key).map_err(SandboxCaError::CertificateBuild)?;
        let ca_not_after = params.not_after;
        let crl_der = empty_crl(ca_cert.der(), &ca_key)?;
        let issuer = Issuer::new(params, ca_key);

        Ok(Self {
            ca_cert: CaCertificate::Generated(ca_cert),
            issuer,
            leaf_key: OnceLock::new(),
            ca_not_after,
            leaves: Mutex::new(LeafCache::default()),
            permitted,
            any_name: false,
            crl_der,
            crl_url: OnceLock::new(),
        })
    }

    /// A CA signing with a caller supplied certificate and key, validated as
    /// [`validate_ca_pair`] does. `path` only labels errors.
    pub fn from_pem_pair(
        cert_pem: &str,
        key_pem: &str,
        permitted: PermittedDomains,
        path: &str,
    ) -> Result<Self, SandboxCaError> {
        let invalid = |reason| SandboxCaError::CaPairInvalid {
            path: path.to_string(),
            reason,
        };
        let cert_der = certificate_der(cert_pem).map_err(invalid)?;
        let ca_key = key_pair_from_pem(key_pem).map_err(invalid)?;
        let validated = validate_loaded(&cert_der, &ca_key).map_err(invalid)?;
        let crl_der = empty_crl(&cert_der, &ca_key)?;
        let issuer = Issuer::from_ca_cert_der(&cert_der, ca_key)
            .map_err(|error| invalid(CaPairError::Parse(error.to_string())))?;
        Ok(Self {
            ca_cert: CaCertificate::Supplied {
                der: cert_der.into_owned(),
                pem: cert_pem.to_string(),
            },
            issuer,
            leaf_key: OnceLock::new(),
            ca_not_after: validated.not_after,
            leaves: Mutex::new(LeafCache::default()),
            permitted,
            any_name: false,
            crl_der,
            crl_url: OnceLock::new(),
        })
    }

    /// Reads the PEM files named by `tlsTerminate.caCertPath` and
    /// `tlsTerminate.caKeyPath` and builds the CA from them.
    pub fn from_files(
        cert_path: &std::path::Path,
        key_path: &std::path::Path,
        permitted: PermittedDomains,
    ) -> Result<Self, SandboxCaError> {
        let cert_pem = read_pem_file("tlsTerminate.caCertPath", cert_path, "CERTIFICATE", |text| {
            text.contains("-----BEGIN CERTIFICATE-----")
        })?;
        let key_pem = read_pem_file("tlsTerminate.caKeyPath", key_path, "PRIVATE KEY", |text| {
            text.lines().any(|line| {
                line.strip_prefix("-----BEGIN ")
                    .and_then(|rest| rest.strip_suffix("PRIVATE KEY-----"))
                    .is_some_and(|label| label.chars().all(|c| c.is_ascii_uppercase() || c == ' '))
            })
        })?;
        Self::from_pem_pair(&cert_pem, &key_pem, permitted, &cert_path.display().to_string())
    }

    pub fn permitted_domains(&self) -> &PermittedDomains {
        &self.permitted
    }

    /// The DER of a revocation list signed by this CA that names no revoked
    /// certificate, which the proxy serves at [`CRL_PATH`].
    pub fn crl_der(&self) -> &[u8] {
        &self.crl_der
    }

    /// Makes every leaf minted from now on name `url` as its CRL distribution
    /// point. Leaves already minted keep what they carry, so this is set
    /// before the first connection. The first URL set stays.
    pub fn set_crl_url(&self, url: CrlUrl) {
        let _ = self.crl_url.set(url);
    }

    /// Mints a leaf for any name or IP literal instead of only the permitted
    /// ones, for a supplied CA whose own constraints are the operator's.
    pub fn permitting_every_name(mut self) -> Self {
        self.any_name = true;
        self
    }

    pub fn mint_leaf_cert(&self, hostname: &str) -> Result<rustls::sign::CertifiedKey, SandboxCaError> {
        if !self.any_name && !self.permitted.permits(hostname) {
            return Err(SandboxCaError::HostnameNotPermitted {
                hostname: hostname.to_string(),
            });
        }
        let key = hostname.to_ascii_lowercase();
        let mut leaves = self.leaves.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(leaf) = leaves.get(&key) {
            return Ok(leaf);
        }
        let leaf = self.sign_leaf(hostname)?;
        leaves.insert(&key, leaf.clone());
        Ok(leaf)
    }

    /// The one RSA-2048 key every leaf of this CA uses, made on first use, so a new host name costs a signature and
    /// not a key generation.
    fn leaf_key(&self) -> Result<&KeyPair, SandboxCaError> {
        if let Some(key) = self.leaf_key.get() {
            return Ok(key);
        }
        let generated = generate_rsa_key_pair()?;
        Ok(self.leaf_key.get_or_init(|| generated))
    }

    fn sign_leaf(&self, hostname: &str) -> Result<rustls::sign::CertifiedKey, SandboxCaError> {
        let leaf_key = self.leaf_key()?;

        let build_failed = |source| SandboxCaError::LeafCertificateBuild {
            hostname: hostname.to_string(),
            source,
        };
        let mut params = CertificateParams::new(vec![hostname.to_string()]).map_err(build_failed)?;
        params.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, hostname);
            dn
        };
        params.subject_alt_names = vec![sni_to_san(hostname)?];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        if let Some(url) = self.crl_url.get() {
            params.crl_distribution_points = vec![CrlDistributionPoint {
                uris: vec![url.as_str().to_string()],
            }];
        }
        let not_before = time::OffsetDateTime::now_utc() - time::Duration::days(BACKDATE_DAYS);
        params.not_before = not_before;
        params.not_after = (not_before + time::Duration::days(LEAF_LIFETIME_DAYS)).min(self.ca_not_after);

        let leaf_cert = params.signed_by(leaf_key, &self.issuer).map_err(build_failed)?;

        let chain = vec![
            rustls::pki_types::CertificateDer::from(leaf_cert.der().to_vec()),
            rustls::pki_types::CertificateDer::from(self.ca_cert.der().to_vec()),
        ];
        let private_key = rustls::pki_types::PrivateKeyDer::from(rustls::pki_types::PrivatePkcs8KeyDer::from(
            leaf_key.serialize_der(),
        ));

        let signing_key =
            rustls::crypto::aws_lc_rs::sign::any_supported_type(&private_key).map_err(SandboxCaError::CertifiedKey)?;

        Ok(rustls::sign::CertifiedKey::new(chain, signing_key))
    }

    pub fn root_cert_pem(&self) -> String {
        self.ca_cert.pem()
    }
}

/// Where a minted leaf says its revocation list can be fetched.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CrlUrl(String);

impl CrlUrl {
    /// The list as the proxy serves it on the loopback address of `port`.
    pub fn loopback(port: u16) -> Self {
        Self(format!("http://127.0.0.1:{port}{CRL_PATH}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn der_time(at: time::OffsetDateTime) -> Vec<u8> {
    let at = at.to_offset(time::UtcOffset::UTC);
    let (tag, year) = if at.year() <= UTC_TIME_LAST_YEAR {
        (DER_UTC_TIME, format!("{:02}", at.year() % 100))
    } else {
        (DER_GENERALIZED_TIME, format!("{:04}", at.year()))
    };
    let text = format!(
        "{year}{:02}{:02}{:02}{:02}{:02}Z",
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    );
    der_tlv(tag, text.as_bytes())
}

/// The RFC 5280 `CertificateList` that names no revoked certificate, signed by
/// `key`, which holds the certificate's public key: the form a
/// revocation client expects. It carries the authority key identifier and the
/// CRL number, and expires with the CA, or a day from now when the CA already
/// has. Built by hand so a CA whose key usage omits `cRLSign` still gets one,
/// as a client expects.
fn empty_crl(cert_der: &[u8], key: &KeyPair) -> Result<Vec<u8>, SandboxCaError> {
    let fail = |why: &dyn std::fmt::Display| SandboxCaError::CrlBuild(why.to_string());
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(cert_der).map_err(|e| fail(&e))?;

    let algorithm = parsed_public_key_algorithm(&key.subject_public_key_info()).map_err(|error| fail(&error))?;
    let signature_algorithm = if algorithm == RSA_ENCRYPTION_OID {
        der_tlv(DER_SEQUENCE, &[OID_SHA256_WITH_RSA, DER_NULL].concat())
    } else {
        der_tlv(DER_SEQUENCE, OID_ECDSA_WITH_SHA256)
    };

    let key_identifier = cert
        .iter_extensions()
        .find_map(|extension| match extension.parsed_extension() {
            x509_parser::extensions::ParsedExtension::SubjectKeyIdentifier(id) => Some(id.0.to_vec()),
            _ => None,
        })
        .unwrap_or_else(|| sha1::Sha1::digest(cert.public_key().subject_public_key.data.as_ref()).to_vec());
    let extension =
        |oid: &[u8], value: Vec<u8>| der_tlv(DER_SEQUENCE, &[oid, &der_tlv(DER_OCTET_STRING, &value)].concat());
    let extensions = der_tlv(
        DER_EXTENSIONS,
        &der_tlv(
            DER_SEQUENCE,
            &[
                extension(
                    OID_AUTHORITY_KEY_IDENTIFIER,
                    der_tlv(DER_SEQUENCE, &der_tlv(DER_KEY_IDENTIFIER, &key_identifier)),
                ),
                extension(OID_CRL_NUMBER, der_tlv(DER_INTEGER, &[1])),
            ]
            .concat(),
        ),
    );

    let now = time::OffsetDateTime::now_utc();
    let ca_end = cert.validity().not_after.to_datetime();
    let next_update = if ca_end > now {
        ca_end
    } else {
        now + time::Duration::days(1)
    };
    let to_be_signed = der_tlv(
        DER_SEQUENCE,
        &[
            der_tlv(DER_INTEGER, &[1]),
            signature_algorithm.clone(),
            cert.tbs_certificate.subject.as_raw().to_vec(),
            der_time(now),
            der_time(next_update),
            extensions,
        ]
        .concat(),
    );
    let signature = key.sign(&to_be_signed).map_err(|e| fail(&e))?;
    let signature = der_tlv(DER_BIT_STRING, &[&[0u8][..], &signature].concat());
    Ok(der_tlv(
        DER_SEQUENCE,
        &[to_be_signed, signature_algorithm, signature].concat(),
    ))
}

fn sni_to_san(hostname: &str) -> Result<SanType, SandboxCaError> {
    let bare = hostname
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(hostname);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(SanType::IpAddress(ip));
    }
    if let Ok(ia5) = Ia5String::try_from(hostname.to_string()) {
        Ok(SanType::DnsName(ia5))
    } else {
        Err(SandboxCaError::InvalidHostname {
            hostname: hostname.to_string(),
        })
    }
}

/// Builds a `rustls::ServerConfig` that presents `certified_key` for every
/// connection (no SNI-based cert selection needed, since the caller has
/// already sniffed the SNI and is minting a leaf cert specifically for it).
pub fn server_config_for_leaf(certified_key: rustls::sign::CertifiedKey) -> Arc<rustls::ServerConfig> {
    let resolver = SingleCertResolver(Arc::new(certified_key));
    Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver)),
    )
}

#[derive(Debug)]
struct SingleCertResolver(Arc<rustls::sign::CertifiedKey>);

impl rustls::server::ResolvesServerCert for SingleCertResolver {
    fn resolve(&self, _client_hello: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

#[cfg(test)]
impl PermittedDomains {
    pub(crate) fn for_tests(patterns: &[&str]) -> Self {
        Self(
            patterns
                .iter()
                .filter_map(|pattern| DomainPattern::parse(pattern).ok())
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests;
