use std::ffi::OsString;
use std::path::PathBuf;

use crate::constants::{PEM_BEGIN_MARKER, PEM_END_MARKER};

/// The CA that verifies the server: a file path, or (from `PGSSLROOTCERT` only,
/// as SQLx reads it) the PEM text itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostgresRootCert {
    File(PathBuf),
    Pem(String),
}

impl PostgresRootCert {
    pub(super) fn from_env(value: OsString) -> Self {
        match value.to_str().map(str::trim) {
            Some(text) if text.starts_with(PEM_BEGIN_MARKER) && text.ends_with(PEM_END_MARKER) => {
                Self::Pem(value.to_string_lossy().into_owned())
            }
            _ => Self::File(PathBuf::from(value)),
        }
    }
}
