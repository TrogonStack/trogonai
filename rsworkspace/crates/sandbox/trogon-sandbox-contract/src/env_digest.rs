use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{Digest, DigestError};

/// Identity of the execution environment a worker runs: VMM, kernel, boot inputs, and snapshot
/// format.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvDigest(Digest);

impl EnvDigest {
    /// OCI annotation key that carries the digest on a published snapshot manifest and in its signed payload.
    pub const ANNOTATION_KEY: &'static str = "io.trogon.sandbox.env-digest";

    pub fn new(value: impl Into<String>) -> Result<Self, DigestError> {
        Digest::new(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl From<Digest> for EnvDigest {
    fn from(digest: Digest) -> Self {
        Self(digest)
    }
}

impl FromStr for EnvDigest {
    type Err = DigestError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for EnvDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
