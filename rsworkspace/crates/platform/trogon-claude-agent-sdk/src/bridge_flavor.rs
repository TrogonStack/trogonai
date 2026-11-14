//! The path syntax behind the upstream path checks.
//!
//! The upstream bridge builds its separator, root and split patterns when it loads, from `process.platform`, and uses
//! the `path` module of that platform. [`Flavor`] carries the same choices: POSIX text rules everywhere but Windows,
//! where drive letters, backslashes and UNC roots count. The share, device and dot segment checks below do not depend
//! on the platform: upstream applies them to every path text it is given.

use crate::bridge_platform::Platform;
use crate::node_path::{JsString, NodeError, NodePath, NodePathDialect, NodeProcess, js, rust};

/// A process whose working directory has no drive, so nothing but the given text decides a Windows answer.
struct DrivelessProcess;

impl NodeProcess for DrivelessProcess {
    fn cwd(&self) -> Result<JsString, NodeError> {
        Ok(js("/"))
    }

    fn env(&self, _key: &[u16]) -> Option<JsString> {
        None
    }
}

/// The path module and the load time patterns of one platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flavor {
    Posix,
    Win32,
}

fn separator_byte(byte: u8) -> bool {
    byte == b'/' || byte == b'\\'
}

fn separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// The length of the root `^(?:[/\\]{2}[^/\\]+[/\\][^/\\]+[/\\]?|[A-Za-z]:[/\\]|[/\\])` matches, or 0.
fn win32_root_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && separator_byte(bytes[0]) && separator_byte(bytes[1]) {
        let server = bytes[2..].iter().position(|byte| separator_byte(*byte));
        if let Some(server) = server.filter(|length| *length > 0) {
            let start = 2 + server + 1;
            let share = bytes[start..]
                .iter()
                .position(|byte| separator_byte(*byte))
                .unwrap_or(bytes.len() - start);
            if share > 0 {
                let end = start + share;
                return end + usize::from(end < bytes.len());
            }
        }
    }
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && separator_byte(bytes[2]) {
        return 3;
    }
    usize::from(bytes.first().is_some_and(|byte| separator_byte(*byte)))
}

fn node_path<T>(use_path: impl FnOnce(&NodePath<'_>) -> T) -> T {
    let process = DrivelessProcess;
    use_path(&NodePath {
        dialect: NodePathDialect::Current,
        windows_host: false,
        process: &process,
    })
}

/// `path.win32.normalize(text)`.
fn win32_normalize(text: &str) -> String {
    node_path(|path| rust(&path.win32_normalize(&js(text))))
}

impl Flavor {
    /// The flavor of the platform the process reports.
    pub(crate) fn of(platform: Platform) -> Self {
        if platform == Platform::Win32 {
            Self::Win32
        } else {
            Self::Posix
        }
    }

    /// `path.sep`.
    pub(crate) fn sep(self) -> char {
        match self {
            Self::Posix => '/',
            Self::Win32 => '\\',
        }
    }

    /// Whether the text starts at a root: a rooted path, or on Windows also a drive letter with a separator.
    pub(crate) fn rooted(self, text: &str) -> bool {
        match self {
            Self::Posix => text.starts_with('/'),
            Self::Win32 => {
                let bytes = text.as_bytes();
                bytes.first().is_some_and(|byte| separator_byte(*byte))
                    || (bytes.len() >= 3
                        && bytes[0].is_ascii_alphabetic()
                        && bytes[1] == b':'
                        && separator_byte(bytes[2]))
            }
        }
    }

    /// The length of the root the walker starts from.
    pub(crate) fn root_len(self, text: &str) -> usize {
        match self {
            Self::Posix => usize::from(text.starts_with('/')),
            Self::Win32 => win32_root_len(text),
        }
    }

    /// Splits where a run of separators is.
    pub(crate) fn split(self, text: &str) -> Vec<&str> {
        split_runs(text, self == Self::Win32)
    }

    /// Whether the text ends with a separator.
    pub(crate) fn ends_with_separator(self, text: &str) -> bool {
        match self {
            Self::Posix => text.ends_with('/'),
            Self::Win32 => text.ends_with(separator),
        }
    }

    /// `path.join(base, name)`.
    pub(crate) fn join(self, base: &str, name: &str) -> String {
        match self {
            Self::Posix => join(base, name),
            Self::Win32 => node_path(|path| rust(&path.win32_join(&[&js(base), &js(name)]))),
        }
    }

    /// `path.dirname(path)`.
    pub(crate) fn dirname(self, text: &str) -> String {
        match self {
            Self::Posix => dirname(text),
            Self::Win32 => node_path(|path| rust(&path.win32_dirname(&js(text)))),
        }
    }

    /// `path.resolve(base, text)`.
    pub(crate) fn resolve(self, base: &str, text: &str) -> String {
        match self {
            Self::Posix => resolve(base, text),
            Self::Win32 => node_path(|path| {
                path.win32_resolve(&[&js(base), &js(text)])
                    .map(|resolved| rust(&resolved))
                    .unwrap_or_default()
            }),
        }
    }

    /// `path.resolve(text)`.
    pub(crate) fn resolve_alone(self, text: &str) -> String {
        match self {
            Self::Posix => resolve(text, "."),
            Self::Win32 => node_path(|path| {
                path.win32_resolve(&[&js(text)])
                    .map(|resolved| rust(&resolved))
                    .unwrap_or_default()
            }),
        }
    }
}

/// `/^[\\/]{2}/`: the text starts with two separators of either kind.
fn two_separators(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2 && separator_byte(bytes[0]) && separator_byte(bytes[1])
}

/// `/^[\\/]{2}[?.][\\/]/`: a Win32 device namespace.
fn device_prefix(text: &str) -> bool {
    let bytes = text.as_bytes();
    two_separators(text) && bytes.len() >= 4 && matches!(bytes[2], b'?' | b'.') && separator_byte(bytes[3])
}

/// `/^[\\/]\?\?[\\/]/`.
fn nt_root(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 4 && separator_byte(bytes[0]) && bytes[1] == b'?' && bytes[2] == b'?' && separator_byte(bytes[3])
}

/// The NT object namespace prefix, also once the text is normalized the way Windows would.
fn nt_prefix(text: &str) -> bool {
    nt_root(text) || (text.contains("??") && nt_root(&win32_normalize(text)))
}

/// `/(^|[\\/])\.{1,2}[. ]*([\\/]|$)/`: a segment made of dots, which Windows trims to `.` or `..`.
fn dot_segment(text: &str) -> bool {
    let bytes = text.as_bytes();
    (0..bytes.len()).any(|start| {
        (start == 0 || separator_byte(bytes[start - 1])) && bytes[start] == b'.' && {
            let end = start
                + bytes[start..]
                    .iter()
                    .take_while(|byte| matches!(**byte, b'.' | b' '))
                    .count();
            end == bytes.len() || separator_byte(bytes[end])
        }
    })
}

/// The lower cased server of a UNC path, when the text names one.
fn server_name(text: &str) -> Option<String> {
    if !two_separators(text) {
        return None;
    }
    let rest = &text[2..];
    let bytes = rest.as_bytes();
    let device = bytes.len() >= 2 && matches!(bytes[0], b'?' | b'.') && separator_byte(bytes[1]);
    let unc = device && bytes.len() >= 6 && bytes[2..5].eq_ignore_ascii_case(b"unc") && separator_byte(bytes[5]);
    if device && !unc {
        return None;
    }
    let first_name = |text: &str| {
        let end = text.find(separator).unwrap_or(text.len());
        (end > 0).then(|| text[..end].to_ascii_lowercase())
    };
    if unc {
        return first_name(&rest[6..]).or_else(|| first_name(rest));
    }
    first_name(rest)
}

/// Whether text that names a Windows share, a device or a dotted segment is refused relative to `base`: the text
/// reaches a network share other than the one `base` is on, or a device namespace, or hides a dot segment.
pub(crate) fn share_refused(text: &str, base: &str) -> bool {
    if nt_prefix(text) {
        return true;
    }
    if !two_separators(text) {
        return false;
    }
    if device_prefix(text) || dot_segment(text) {
        return true;
    }
    let name = server_name(text);
    name.is_none() || name != server_name(base)
}

/// `path.posix.dirname(path)`.
pub(crate) fn dirname(path: &str) -> String {
    node_path(|node| rust(&node.posix_dirname(&js(path))))
}

/// `path.posix.join(base, name)`.
pub(crate) fn join(base: &str, name: &str) -> String {
    node_path(|node| rust(&node.posix_join(&[&js(base), &js(name)])))
}

/// `path.posix.resolve(base, text)` for an absolute `base`.
pub(crate) fn resolve(base: &str, text: &str) -> String {
    let combined = if text.starts_with('/') {
        text.to_owned()
    } else {
        format!("{base}/{text}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in combined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `text.split(/\/+/)`, or `text.split(/[/\\]+/)` on Windows: runs of separators split, and a trailing run leaves one
/// empty piece.
pub(crate) fn split_runs(text: &str, windows: bool) -> Vec<&str> {
    let separates = |byte: u8| byte == b'/' || (windows && byte == b'\\');
    let mut pieces = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if separates(bytes[index]) {
            pieces.push(&text[start..index]);
            while index < bytes.len() && separates(bytes[index]) {
                index += 1;
            }
            start = index;
        } else {
            index += 1;
        }
    }
    pieces.push(&text[start..]);
    pieces
}

#[cfg(test)]
#[path = "bridge_flavor_tests.rs"]
mod tests;
