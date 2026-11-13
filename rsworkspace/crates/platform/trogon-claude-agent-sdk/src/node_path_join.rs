//! Node's `path.join` for both platforms, which `extractFromBunfs` uses to build every path it touches. It joins
//! the parts and then normalizes lexically: separators collapse, `.` goes away and `..` pops a segment.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PathStyle {
    Posix,
    Win32,
}

impl PathStyle {
    /// The style Node's `path` module has on the platform this crate was built for.
    pub(crate) const fn host() -> Self {
        if cfg!(windows) { Self::Win32 } else { Self::Posix }
    }
}

fn is_win32_separator(byte: u8) -> bool {
    byte == b'/' || byte == b'\\'
}

/// `path.posix.join` or `path.win32.join`: empty parts are skipped and nothing left gives `.`.
pub(crate) fn join(style: PathStyle, parts: &[&str]) -> String {
    let parts: Vec<&str> = parts.iter().copied().filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return ".".to_owned();
    }
    match style {
        PathStyle::Posix => normalize_posix(&parts.join("/")),
        PathStyle::Win32 => join_win32(&parts),
    }
}

/// Node's `normalizeString`: drops empty and `.` segments, lets `..` pop the previous segment, and keeps a leading
/// `..` only when the path may climb above its root.
fn normalize_segments(path: &str, allow_above_root: bool, separator: &str, is_separator: fn(char) -> bool) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for segment in path.split(is_separator) {
        match segment {
            "" | "." => {}
            ".." if kept.last().is_some_and(|last| *last != "..") => {
                kept.pop();
            }
            ".." if allow_above_root => kept.push(".."),
            ".." => {}
            other => kept.push(other),
        }
    }
    kept.join(separator)
}

fn normalize_posix(path: &str) -> String {
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut normalized = normalize_segments(path, !absolute, "/", |c| c == '/');
    if normalized.is_empty() {
        return match (absolute, trailing) {
            (true, _) => "/".to_owned(),
            (false, true) => "./".to_owned(),
            (false, false) => ".".to_owned(),
        };
    }
    if trailing {
        normalized.push('/');
    }
    if absolute { format!("/{normalized}") } else { normalized }
}

/// `path.win32.join`: a first part that starts with a single separator, or with a drive, is joined as is; one that
/// starts with two separators and a server name is a UNC root and keeps both; any other run of leading
/// separators shrinks to one.
fn join_win32(parts: &[&str]) -> String {
    let mut joined = parts.join("\\");
    let first = parts[0].as_bytes();
    let mut slash_count = 0;
    let mut needs_replace = true;
    if first.first().copied().is_some_and(is_win32_separator) {
        slash_count = 1;
        if first.get(1).copied().is_some_and(is_win32_separator) {
            slash_count = 2;
            if first.len() > 2 {
                if is_win32_separator(first[2]) {
                    slash_count = 3;
                } else {
                    needs_replace = false;
                }
            }
        }
    }
    if needs_replace {
        let bytes = joined.as_bytes();
        while slash_count < bytes.len() && is_win32_separator(bytes[slash_count]) {
            slash_count += 1;
        }
        if slash_count >= 2 {
            joined = format!("\\{}", &joined[slash_count..]);
        }
    }
    normalize_win32(&joined)
}

fn normalize_win32(path: &str) -> String {
    let bytes = path.as_bytes();
    let len = bytes.len();
    if len == 1 {
        return if is_win32_separator(bytes[0]) {
            "\\".to_owned()
        } else {
            path.to_owned()
        };
    }
    let mut root_end = 0;
    let mut device: Option<String> = None;
    let mut absolute = false;
    if is_win32_separator(bytes[0]) {
        absolute = true;
        if is_win32_separator(bytes[1]) {
            let mut j = 2;
            let mut last = j;
            while j < len && !is_win32_separator(bytes[j]) {
                j += 1;
            }
            if j < len && j != last {
                let server = &path[last..j];
                last = j;
                while j < len && is_win32_separator(bytes[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !is_win32_separator(bytes[j]) {
                        j += 1;
                    }
                    if j == len {
                        return format!("\\\\{server}\\{}\\", &path[last..]);
                    }
                    if j != last {
                        device = Some(format!("\\\\{server}\\{}", &path[last..j]));
                        root_end = j;
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else if bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        device = Some(path[..2].to_owned());
        root_end = 2;
        if len > 2 && is_win32_separator(bytes[2]) {
            absolute = true;
            root_end = 3;
        }
    }
    let mut tail = if root_end < len {
        normalize_segments(&path[root_end..], !absolute, "\\", |c| c == '/' || c == '\\')
    } else {
        String::new()
    };
    if tail.is_empty() && !absolute {
        tail = ".".to_owned();
    }
    if !tail.is_empty() && is_win32_separator(bytes[len - 1]) {
        tail.push('\\');
    }
    match (device, absolute) {
        (None, true) => format!("\\{tail}"),
        (None, false) => tail,
        (Some(device), true) => format!("{device}\\{tail}"),
        (Some(device), false) => format!("{device}{tail}"),
    }
}

#[cfg(test)]
#[path = "node_path_join_tests.rs"]
mod tests;
