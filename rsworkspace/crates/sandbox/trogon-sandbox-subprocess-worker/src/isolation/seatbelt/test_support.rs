//! Builders the unit tests of the Seatbelt backend share: policy entries from
//! raw text and a policy assembled from named lists.

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, GlobPattern, LiteralPath};

pub type TestError = Box<dyn std::error::Error>;

pub fn entry(raw: &str) -> Result<FsPathEntry, TestError> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::Unmarked,
    })
}

pub fn glob(raw: &str) -> Result<FsPathEntry, TestError> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

#[derive(Default, Clone)]
pub struct Lists {
    pub deny_read: Vec<FsPathEntry>,
    pub credential_deny_read: Vec<FsPathEntry>,
    pub allow_read_within_deny: Vec<FsPathEntry>,
    pub own_allow_within_deny: Vec<FsPathEntry>,
    pub allow_write: Vec<FsPathEntry>,
    pub deny_write: Vec<FsPathEntry>,
}

impl Lists {
    pub fn build(self) -> Result<FsPolicy, TestError> {
        Ok(FsPolicy::new(
            self.deny_read,
            self.credential_deny_read,
            self.allow_read_within_deny,
            self.own_allow_within_deny,
            self.allow_write,
            self.deny_write,
        )?)
    }
}

pub fn lp(raw: &str) -> Result<LiteralPath, TestError> {
    Ok(LiteralPath::new(raw)?)
}
