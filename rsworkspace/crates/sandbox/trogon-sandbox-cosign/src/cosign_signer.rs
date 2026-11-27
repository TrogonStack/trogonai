use std::path::PathBuf;

use secrecy::{SecretBox, SecretString};

/// Signing identity for snapshot manifests.
#[derive(Debug)]
pub enum CosignSigner {
    /// AWS KMS key URI, e.g. `awskms:///arn:aws:kms:us-east-1:123456789012:key/abc`, signed by
    /// running the `cosign` program.
    AwsKms { uri: String, program: PathBuf },
    /// Local PEM private key file (password optional). Used in tests and dev only.
    LocalKey {
        private_key_path: PathBuf,
        password: Option<SecretString>,
    },
    /// In-memory PEM bytes (tests).
    InMemoryKey {
        private_key_pem: SecretBox<Vec<u8>>,
        password: Option<SecretString>,
    },
}

impl CosignSigner {
    /// A KMS signer that runs `cosign` from `PATH`.
    pub fn from_kms_uri(uri: impl Into<String>) -> Self {
        Self::AwsKms {
            uri: uri.into(),
            program: PathBuf::from("cosign"),
        }
    }

    /// A KMS signer that runs the given `cosign` executable.
    pub fn from_kms_uri_with_program(uri: impl Into<String>, program: impl Into<PathBuf>) -> Self {
        Self::AwsKms {
            uri: uri.into(),
            program: program.into(),
        }
    }

    pub fn from_private_key_path(path: impl Into<PathBuf>, password: Option<SecretString>) -> Self {
        Self::LocalKey {
            private_key_path: path.into(),
            password,
        }
    }

    pub fn from_private_key_pem(private_key_pem: Vec<u8>, password: Option<SecretString>) -> Self {
        Self::InMemoryKey {
            private_key_pem: SecretBox::new(Box::new(private_key_pem)),
            password,
        }
    }
}
