use std::path::Path;

use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionProjectKey(String);

#[derive(Debug, thiserror::Error)]
pub enum SessionProjectKeyError {
    #[error("invalid session project key")]
    Invalid,
    #[error("could not resolve the project directory")]
    Directory(#[source] std::io::Error),
    #[error("project directory is not Unicode")]
    NotUnicode,
}

impl SessionProjectKey {
    pub fn new(value: impl Into<String>) -> Result<Self, SessionProjectKeyError> {
        let value = value.into();
        if !crate::session_path_component::is_safe(&value) {
            return Err(SessionProjectKeyError::Invalid);
        }
        Ok(Self(value))
    }

    pub fn for_directory(directory: &Path) -> Result<Self, SessionProjectKeyError> {
        Self::for_directory_with_filesystem(directory, &crate::adapters::NativeFileSystem)
    }

    pub fn for_directory_with_filesystem<F: crate::adapters::CanonicalizePath<Error = std::io::Error>>(
        directory: &Path,
        filesystem: &F,
    ) -> Result<Self, SessionProjectKeyError> {
        let absolute = filesystem
            .canonicalize(directory)
            .map_err(SessionProjectKeyError::Directory)?;
        let absolute = absolute.to_str().ok_or(SessionProjectKeyError::NotUnicode)?;
        let normalized = platform_nfc(&without_verbatim_prefix(absolute), crate::SummaryDialect::TypeScript);
        Self::new(sanitize_path(&normalized, crate::SummaryDialect::TypeScript))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Windows canonicalization returns verbatim paths (`\\?\C:\dir`, `\\?\UNC\srv\share`),
/// while the CLI and the upstream SDKs derive the key from a realpath without that prefix.
fn without_verbatim_prefix(path: &str) -> std::borrow::Cow<'_, str> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return std::borrow::Cow::Owned(format!(r"\\{rest}"));
    }
    std::borrow::Cow::Borrowed(path.strip_prefix(r"\\?\").unwrap_or(path))
}

impl std::fmt::Display for SessionProjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Upstream `sanitizePath` / `_sanitize_path`: every non-alphanumeric unit becomes a hyphen and a name longer than 200
/// characters is cut and suffixed with a base36 hash. TypeScript counts and hashes UTF-16 units, Python code points.
pub(crate) fn sanitize_path(name: &str, dialect: crate::SummaryDialect) -> String {
    let sanitized = hyphenate(name, dialect);
    if sanitized.len() <= 200 {
        return sanitized;
    }
    let hash = match dialect {
        crate::SummaryDialect::TypeScript => name
            .encode_utf16()
            .fold(0_i32, |hash, unit| hash.wrapping_mul(31).wrapping_add(i32::from(unit))),
        crate::SummaryDialect::Python => name
            .chars()
            .fold(0_i32, |hash, ch| hash.wrapping_mul(31).wrapping_add(ch as i32)),
    };
    let mut number = hash.unsigned_abs();
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit(number % 36, 36).unwrap_or('0'));
        number /= 36;
        if number == 0 {
            break;
        }
    }
    let suffix: String = digits.into_iter().rev().collect();
    format!("{}-{suffix}", &sanitized[..200])
}

/// Every non-alphanumeric unit becomes a hyphen, without the length cut: TypeScript replaces each UTF-16 unit (a
/// character outside the BMP becomes two hyphens), Python each code point.
pub(crate) fn hyphenate(name: &str, dialect: crate::SummaryDialect) -> String {
    match dialect {
        crate::SummaryDialect::TypeScript => name
            .encode_utf16()
            .map(|unit| match char::from_u32(u32::from(unit)) {
                Some(ch) if ch.is_ascii_alphanumeric() => ch,
                _ => '-',
            })
            .collect(),
        crate::SummaryDialect::Python => name
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect(),
    }
}

/// Unicode normalization of a path: TypeScript's `tt` applies NFC on macOS only, Python's `_canonicalize_path` on
/// every platform.
pub(crate) fn platform_nfc(text: &str, dialect: crate::SummaryDialect) -> String {
    match dialect {
        crate::SummaryDialect::TypeScript if !cfg!(target_os = "macos") => text.to_owned(),
        _ => text.nfc().collect(),
    }
}

/// Upstream `_canonicalize_path`: realpath plus NFC, falling back to the input when the path cannot be resolved.
pub(crate) fn canonicalize_lenient(directory: &Path) -> String {
    canonicalize_lenient_in(directory, crate::SummaryDialect::Python)
}

/// Upstream `canonicalizePath` / `_canonicalize_path` for the selected rule set.
pub(crate) fn canonicalize_lenient_in(directory: &Path, dialect: crate::SummaryDialect) -> String {
    let resolved = directory
        .canonicalize()
        .unwrap_or_else(|_| std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf()));
    let text = resolved.to_string_lossy();
    platform_nfc(&without_verbatim_prefix(&text), dialect)
}

/// Derives the [`SessionStore`](crate::SessionStore) project key for a directory (the current directory when
/// `None`), with upstream's realpath, NFC and hashed sanitization rules, so keys match local transcripts.
pub fn project_key_for_directory(directory: Option<&Path>) -> Result<SessionProjectKey, SessionProjectKeyError> {
    project_key_for_directory_in(directory, crate::SummaryDialect::TypeScript)
}

/// [`project_key_for_directory`] with a selectable upstream rule set. The two differ for a directory whose sanitized
/// name exceeds 200 characters and contains characters outside the Basic Multilingual Plane: TypeScript counts and
/// hashes UTF-16 units, Python code points.
pub fn project_key_for_directory_in(
    directory: Option<&Path>,
    dialect: crate::SummaryDialect,
) -> Result<SessionProjectKey, SessionProjectKeyError> {
    let directory = directory.unwrap_or_else(|| Path::new("."));
    SessionProjectKey::new(sanitize_path(&canonicalize_lenient_in(directory, dialect), dialect))
}
