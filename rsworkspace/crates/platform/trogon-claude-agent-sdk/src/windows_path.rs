//! Windows path rules as text, so they run on any host: Node `path.win32` and Python `ntpath` semantics for the
//! transcript-path and resume-subpath checks. Both `/` and `\` separate components, a drive (`C:`) or a UNC share
//! (`\\server\share`) is the device, and `..` never climbs above the root.

/// The device (`C:` or `\\server\share`, empty when absent) and the rest of the path with `\` separators.
fn split_device(path: &str) -> (String, String) {
    let path = path.replace('/', "\\");
    if let Some(unc) = path.strip_prefix("\\\\") {
        let mut names = unc.split('\\');
        let server = names.next().unwrap_or_default();
        let share = names.next().unwrap_or_default();
        if server.is_empty() || share.is_empty() {
            return (String::new(), path);
        }
        let device = format!("\\\\{server}\\{share}");
        let rest = path[device.len()..].to_string();
        return (device, rest);
    }
    let mut chars = path.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic);
    if drive.is_some() && chars.next() == Some(':') {
        return (path[..2].to_string(), path[2..].to_string());
    }
    (String::new(), path)
}

/// Node `path.win32.resolve(base, path)` as the device followed by its normalized components. `base` is absolute.
/// A drive-relative `path` on the base's drive keeps its own spelling of the drive, as Node takes the device from the
/// rightmost argument that names one.
pub(crate) fn resolve(base: &str, path: &str) -> Vec<String> {
    let (base_device, base_rest) = split_device(base);
    let (device, rest) = split_device(path);
    let below_base = |rest: &str| format!("{}\\{rest}", base_rest.trim_end_matches('\\'));
    let (device, rest) = if rest.starts_with('\\') && !device.is_empty() {
        (device, rest)
    } else if rest.starts_with('\\') {
        (base_device, rest)
    } else if device.is_empty() {
        (base_device, below_base(&rest))
    } else if device.to_lowercase() == base_device.to_lowercase() {
        (device, below_base(&rest))
    } else {
        (device, format!("\\{rest}"))
    };
    let mut parts = vec![device];
    for part in rest.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            name => parts.push(name.to_string()),
        }
    }
    parts
}

/// The resolved path as Node prints it: the device, a separator and the components.
pub(crate) fn resolve_to_string(base: &str, path: &str) -> String {
    let parts = resolve(base, path);
    format!("{}\\{}", parts[0], parts[1..].join("\\"))
}

/// Node `path.win32.isAbsolute`: a leading separator, or a drive letter and colon followed by a separator.
pub(crate) fn is_absolute(path: &str) -> bool {
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some('/' | '\\'), ..) => true,
        (Some(drive), Some(':'), Some('/' | '\\')) => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

/// The components of `path` below `base`, compared case-insensitively on Unicode lowercase, or `None` when `path`
/// is not inside `base`. The components keep the case of `path`. Relative paths resolve against `cwd`.
pub(crate) fn components_below(base: &str, path: &str, cwd: &str) -> Option<Vec<String>> {
    let base = resolve(cwd, base);
    let path = resolve(cwd, path);
    let common = base
        .iter()
        .zip(&path)
        .take_while(|(left, right)| left.to_lowercase() == right.to_lowercase())
        .count();
    (common == base.len()).then(|| path[common..].to_vec())
}

/// The parts Python 0.2.181 reads from the relative path on CPython 3.12, where `ntpath.relpath` ends with
/// `ntpath.join(*parts)` and `PureWindowsPath(...).parts` splits the result. `join` reads any character followed by a
/// colon at the start of a part as a drive: a part on another drive restarts the path, the same drive in another case
/// replaces the drive, and a drive is dropped from the parts after it. The drive then comes back as the first part.
pub(crate) fn python_relative_parts(parts: &[String]) -> Vec<String> {
    let mut drive = String::new();
    let mut path = String::new();
    for (index, part) in parts.iter().enumerate() {
        let (part_drive, part_path) = split_colon_drive(part);
        if index == 0 {
            drive = part_drive.to_owned();
            path = part_path.to_owned();
            continue;
        }
        if !part_drive.is_empty() && part_drive != drive {
            let restart = part_drive.to_lowercase() != drive.to_lowercase();
            drive = part_drive.to_owned();
            if restart {
                path = part_path.to_owned();
                continue;
            }
        }
        if !path.is_empty() && !path.ends_with('\\') {
            path.push('\\');
        }
        path.push_str(part_path);
    }
    let mut out = Vec::new();
    if !drive.is_empty() {
        out.push(drive);
    }
    out.extend(path.split('\\').filter(|part| !part.is_empty()).map(str::to_owned));
    out
}

/// `ntpath.splitroot` on a part with no separator: any character followed by a colon is the drive.
fn split_colon_drive(part: &str) -> (&str, &str) {
    let mut chars = part.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(':')) => part.split_at(first.len_utf8() + 1),
        _ => ("", part),
    }
}
