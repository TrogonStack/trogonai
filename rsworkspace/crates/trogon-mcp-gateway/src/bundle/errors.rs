use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleLoadError {
    Archive(String),
    ManifestMissing,
    ManifestParse(String),
    ManifestInvalid(String),
    SignatureMissing,
    SignatureMalformed(String),
    SignatureInvalid,
    SignatureVerificationUnavailable {
        reason: &'static str,
    },
    UntrustedSigner {
        nkey_pub: String,
    },
    ContentHashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    UnknownMember {
        path: String,
    },
    MemberMissing {
        path: String,
    },
    MemberTooLarge {
        path: String,
        size: usize,
        limit: usize,
    },
    ArchiveTooLarge {
        size: usize,
        limit: usize,
    },
    UnsupportedTargetWit {
        declared: String,
        supported: String,
    },
    GatewayTooOld {
        min_gateway_version: String,
        running: String,
    },
    DeprecatedManifestFilename {
        used: String,
    },
}

impl fmt::Display for BundleLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(detail) => write!(f, "bundle archive error: {detail}"),
            Self::ManifestMissing => f.write_str("manifest.toml missing from bundle"),
            Self::ManifestParse(detail) => write!(f, "manifest parse error: {detail}"),
            Self::ManifestInvalid(detail) => write!(f, "manifest validation error: {detail}"),
            Self::SignatureMissing => f.write_str("signatures/manifest.sig missing from bundle"),
            Self::SignatureMalformed(detail) => write!(f, "manifest signature malformed: {detail}"),
            Self::SignatureInvalid => f.write_str("manifest NKey signature verification failed"),
            Self::SignatureVerificationUnavailable { reason } => {
                write!(f, "manifest signature verification unavailable: {reason}")
            }
            Self::UntrustedSigner { nkey_pub } => {
                write!(f, "signing NKey `{nkey_pub}` not in trusted_signers allowlist")
            }
            Self::ContentHashMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "content hash mismatch for `{path}`: expected `{expected}`, got `{actual}`"
            ),
            Self::UnknownMember { path } => {
                write!(f, "archive contains undeclared member `{path}`")
            }
            Self::MemberMissing { path } => {
                write!(f, "manifest declares missing member `{path}`")
            }
            Self::MemberTooLarge { path, size, limit } => write!(
                f,
                "member `{path}` size {size} exceeds limit {limit}"
            ),
            Self::ArchiveTooLarge { size, limit } => write!(
                f,
                "bundle archive size {size} exceeds limit {limit}"
            ),
            Self::UnsupportedTargetWit {
                declared,
                supported,
            } => write!(
                f,
                "target_wit `{declared}` not supported (host supports `{supported}`)"
            ),
            Self::GatewayTooOld {
                min_gateway_version,
                running,
            } => write!(
                f,
                "min_gateway_version `{min_gateway_version}` exceeds running gateway `{running}`"
            ),
            Self::DeprecatedManifestFilename { used } => write!(
                f,
                "deprecated manifest filename `{used}`; use manifest.toml"
            ),
        }
    }
}

impl std::error::Error for BundleLoadError {}
