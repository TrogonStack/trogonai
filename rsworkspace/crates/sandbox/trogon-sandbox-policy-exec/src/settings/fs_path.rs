//! Path entries built from the contract's path types, with one error for the
//! ways a settings entry can fail to become one.

use trogon_sandbox_contract::{
    DirectoryMarker, FsPathEntry, GlobPattern, GlobPatternError, LiteralPath, LiteralPathError,
};

/// Why a settings entry is not a path entry.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub(super) enum FsPathError {
    #[error("path cannot be empty")]
    Empty,
    #[error(transparent)]
    Literal(#[from] LiteralPathError),
    #[error(transparent)]
    Glob(#[from] GlobPatternError),
}

impl FsPathError {
    /// Whether the text held no wildcard, so it names a path and not a pattern.
    pub(super) fn is_not_a_glob(&self) -> bool {
        matches!(self, Self::Glob(GlobPatternError::NotAGlob(_)))
    }
}

/// The entry that names `raw` as a path, whatever characters it holds.
pub(super) fn literal(raw: &str) -> Result<FsPathEntry, FsPathError> {
    literal_marked(raw, DirectoryMarker::Unmarked)
}

/// Like [`literal`], remembering that the caller wrote a trailing separator.
pub(super) fn literal_directory(raw: &str) -> Result<FsPathEntry, FsPathError> {
    literal_marked(raw, DirectoryMarker::Directory)
}

fn literal_marked(raw: &str, directory: DirectoryMarker) -> Result<FsPathEntry, FsPathError> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory,
    })
}

/// The entry that reads `raw` as a pattern, refused when it holds no wildcard.
pub(super) fn glob(raw: &str) -> Result<FsPathEntry, FsPathError> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

/// The brackets of `raw` that spell no set and so are read as text, as they
/// are written.
pub(super) fn textual_brackets(raw: &str) -> Vec<String> {
    let units: Vec<u16> = raw.encode_utf16().collect();
    let text_of = |part: &[u16]| String::from_utf16_lossy(part);
    let mut found = Vec::new();
    let mut text_until = 0;
    let mut index = 0;
    while index < units.len() {
        if units[index] != u16::from(b'[') || index < text_until {
            index += 1;
            continue;
        }
        let close = units[index + 1..]
            .iter()
            .position(|unit| *unit == u16::from(b']'))
            .map(|offset| index + 1 + offset);
        match close {
            None => {
                found.push(text_of(&units[index..]));
                text_until = units.len();
                index += 1;
            }
            Some(close) if !spells_set(&units[index + 1..close]) => {
                found.push(format!("[{}]", text_of(&units[index + 1..close])));
                text_until = close + 1;
                index += 1;
            }
            Some(close) => index = close + 1,
        }
    }
    found
}

/// Whether the members between brackets spell a set: at least one member, and
/// every range ascending.
fn spells_set(members: &[u16]) -> bool {
    if members.is_empty() {
        return false;
    }
    let dash = u16::from(b'-');
    let mut index = 0;
    while index < members.len() {
        let is_range = members.get(index + 1) == Some(&dash) && index + 2 < members.len();
        let from = members[index];
        if is_range {
            index += 2;
        }
        if members[index] < from {
            return false;
        }
        index += 1;
    }
    true
}
