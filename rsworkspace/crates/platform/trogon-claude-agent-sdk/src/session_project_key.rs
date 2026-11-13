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

    /// Upstream TypeScript `Wn(directory, env)` through a filesystem adapter: the path is resolved lexically, its real
    /// path is taken when that succeeds (the lexical path stays otherwise), and `CLAUDE_CODE_PROJECT_DIR_NAME` names the
    /// project while `config_dir` is set.
    pub(crate) fn for_directory_ts_with_filesystem<F: crate::adapters::CanonicalizePath<Error = std::io::Error>>(
        directory: &Path,
        filesystem: &F,
        config_dir: Option<&str>,
        name_override: Option<&str>,
    ) -> Result<Self, SessionProjectKeyError> {
        let lexical = lexical_resolve(directory);
        let resolved = filesystem.canonicalize(&lexical).unwrap_or(lexical);
        let text = resolved.to_string_lossy();
        let canonical = platform_nfc(&without_verbatim_prefix(&text), crate::SummaryDialect::TypeScript);
        Self::new(project_name_with(&canonical, config_dir, name_override))
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
///
/// TypeScript `CD` resolves the path lexically first (`path.resolve`, which folds `.` and `..` before any symlink is
/// followed) and then takes the real path, keeping the lexical result when that fails. Python's `os.path.realpath`
/// follows symlinks while it walks, so `..` after a symlink leaves the link's target.
pub(crate) fn canonicalize_lenient_in(directory: &Path, dialect: crate::SummaryDialect) -> String {
    let resolved = match dialect {
        crate::SummaryDialect::TypeScript => {
            let lexical = lexical_resolve(directory);
            lexical.canonicalize().unwrap_or(lexical)
        }
        crate::SummaryDialect::Python => directory
            .canonicalize()
            .unwrap_or_else(|_| std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf())),
    };
    let text = resolved.to_string_lossy();
    platform_nfc(&without_verbatim_prefix(&text), dialect)
}

/// Node `path.resolve(directory ?? ".")`: absolute against the current directory, with `.` and `..` folded
/// lexically and no trailing separator.
pub(crate) fn lexical_resolve(directory: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let directory = if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        directory
    };
    let absolute = std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf());
    let mut out = std::path::PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if out.file_name().is_some() {
                    out.pop();
                }
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

/// Upstream `vC`: a project directory name override must be 1 to 64 of `[A-Za-z0-9_-]` and must not be a reserved
/// Windows device name.
fn valid_project_dir_name(name: &str) -> Option<&str> {
    let allowed = (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    let lowered = name.to_ascii_lowercase();
    let reserved = matches!(lowered.as_str(), "con" | "prn" | "aux" | "nul")
        || (lowered.len() == 4
            && (lowered.starts_with("com") || lowered.starts_with("lpt"))
            && lowered.as_bytes()[3].is_ascii_digit());
    (allowed && !reserved).then_some(name)
}

/// Upstream `Wn(directory, env)` once the path is canonical: the override is honored only while `CLAUDE_CONFIG_DIR` is
/// set (non-empty); otherwise, or when the override is invalid, the sanitized path names the project.
pub(crate) fn project_name_with(canonical: &str, config_dir: Option<&str>, name_override: Option<&str>) -> String {
    config_dir
        .filter(|dir| !dir.is_empty())
        .and_then(|_| name_override.and_then(valid_project_dir_name))
        .map_or_else(
            || sanitize_path(canonical, crate::SummaryDialect::TypeScript),
            str::to_owned,
        )
}

/// Upstream `_o` / `Wn(directory)`: [`project_name_with`] over the process environment.
pub(crate) fn project_name_ts(canonical: &str) -> String {
    let config_dir = std::env::var("CLAUDE_CONFIG_DIR").ok();
    let name = std::env::var("CLAUDE_CODE_PROJECT_DIR_NAME").ok();
    project_name_with(canonical, config_dir.as_deref(), name.as_deref())
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
    let canonical = canonicalize_lenient_in(directory, dialect);
    SessionProjectKey::new(match dialect {
        crate::SummaryDialect::TypeScript => project_name_ts(&canonical),
        crate::SummaryDialect::Python => sanitize_path(&canonical, dialect),
    })
}
