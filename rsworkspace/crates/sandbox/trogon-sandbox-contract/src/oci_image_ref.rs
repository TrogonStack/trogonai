use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Digest;
use crate::image_pattern::{split_digest, split_registry, split_tag};

/// An OCI image reference split into registry, repository, optional tag, and optional digest.
///
/// The tag and digest are omitted from JSON when absent.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct OciImageRef {
    pub registry: String,
    pub repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
}

impl OciImageRef {
    /// Splits an image string such as `ghcr.io/team/app:1.0@sha256:...` without resolving
    /// defaults.
    ///
    /// The first path component is the registry only when it contains `.` or `:`, or is
    /// `localhost`; otherwise the registry stays empty. A `@sha256:` suffix is removed and kept as
    /// the digest only when it is a valid [`Digest`].
    pub fn parse_unqualified(value: &str) -> Self {
        let (without_digest, digest) = split_digest(value);
        let (name, tag) = split_tag(without_digest);
        let (registry, repository) = split_registry(name);
        Self {
            registry,
            repository,
            tag: tag.map(str::to_string),
            digest,
        }
    }

    /// The reference as `registry/repository:tag@digest`, leaving out the registry when it is
    /// empty and each suffix when it is absent.
    pub fn allowlist_string(&self) -> String {
        let mut out = if self.registry.is_empty() {
            self.repository.clone()
        } else {
            format!("{}/{}", self.registry, self.repository)
        };
        if let Some(tag) = &self.tag {
            out.push(':');
            out.push_str(tag);
        }
        if let Some(digest) = &self.digest {
            out.push('@');
            out.push_str(digest.as_str());
        }
        out
    }
}

impl fmt::Display for OciImageRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.allowlist_string())
    }
}
