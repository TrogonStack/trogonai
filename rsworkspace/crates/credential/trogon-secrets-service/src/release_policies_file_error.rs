use std::io;
use std::path::PathBuf;

use trogon_credential_domain::CredentialKey;

use super::{ReleasePolicyEntryError, TomlSyntax};

/// Why the release policies file was refused. The service does not start on
/// any of them: a credential whose policy it cannot read would be denied every
/// release anyway, and loading the rest would hide which one is missing.
/// Entries are numbered from 1 in the order the file lists them.
#[derive(Debug, thiserror::Error)]
pub enum ReleasePoliciesFileError {
    #[error("release policies file {} could not be read", path.display())]
    Unreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("release policies file {} is not a valid release policies file: {syntax}", path.display())]
    Unparsable { path: PathBuf, syntax: TomlSyntax },
    #[error("release policies file {} names no policies", path.display())]
    NoPolicies { path: PathBuf },
    #[error("release policy {entry} in release policies file {} is invalid", path.display())]
    InvalidPolicy {
        path: PathBuf,
        entry: usize,
        #[source]
        source: ReleasePolicyEntryError,
    },
    #[error(
        "release policies {first} and {second} in release policies file {} are both for credential {credential}",
        path.display()
    )]
    DuplicatePolicy {
        path: PathBuf,
        credential: CredentialKey,
        first: usize,
        second: usize,
    },
}
