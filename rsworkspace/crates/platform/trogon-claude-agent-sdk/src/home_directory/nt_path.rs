//! CPython 3.12 `ntpath` and the `PureWindowsPath` parse and format steps, ported over code points the way Python
//! indexes strings. Python runs these for `Path.home()` and `Path.home() / segment` on Windows.

const SEPARATOR: char = '\\';

type Split = (String, String, String);

fn slice(chars: &[char], start: usize, end: usize) -> String {
    let end = end.min(chars.len());
    chars[start.min(end)..end].iter().collect()
}

fn find(chars: &[char], needle: char, start: usize) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, character)| **character == needle)
        .map(|(index, _)| index)
}

/// `ntpath.splitroot`: the drive (or UNC share), the root and the rest, which always add back up to the input.
fn splitroot(path: &str) -> Split {
    let chars: Vec<char> = path.chars().collect();
    let normalized: Vec<char> = chars
        .iter()
        .map(|&character| if character == '/' { SEPARATOR } else { character })
        .collect();
    let rest = |from: usize| slice(&chars, from, chars.len());
    if normalized.first() == Some(&SEPARATOR) {
        if normalized.get(1) == Some(&SEPARATOR) {
            let head: String = normalized.iter().take(8).collect();
            let start = if head.to_uppercase() == "\\\\?\\UNC\\" { 8 } else { 2 };
            let Some(index) = find(&normalized, SEPARATOR, start) else {
                return (path.to_owned(), String::new(), String::new());
            };
            let Some(second) = find(&normalized, SEPARATOR, index + 1) else {
                return (path.to_owned(), String::new(), String::new());
            };
            (
                slice(&chars, 0, second),
                slice(&chars, second, second + 1),
                rest(second + 1),
            )
        } else {
            (String::new(), slice(&chars, 0, 1), rest(1))
        }
    } else if normalized.get(1) == Some(&':') {
        if normalized.get(2) == Some(&SEPARATOR) {
            (slice(&chars, 0, 2), slice(&chars, 2, 3), rest(3))
        } else {
            (slice(&chars, 0, 2), String::new(), rest(2))
        }
    } else {
        (String::new(), String::new(), path.to_owned())
    }
}

/// `ntpath.join(path, other)`.
pub(super) fn join(path: &str, other: &str) -> String {
    let (mut drive, mut root, mut rest) = splitroot(path);
    let (other_drive, other_root, other_rest) = splitroot(other);
    let mut replaced = false;
    if !other_root.is_empty() {
        if !other_drive.is_empty() || drive.is_empty() {
            drive = other_drive.clone();
        }
        root = other_root;
        rest = other_rest.clone();
        replaced = true;
    } else if !other_drive.is_empty() && other_drive != drive {
        if other_drive.to_lowercase() != drive.to_lowercase() {
            drive = other_drive.clone();
            root = other_root;
            rest = other_rest.clone();
            replaced = true;
        } else {
            drive = other_drive.clone();
        }
    }
    if !replaced {
        if rest.chars().next_back().is_some_and(|last| last != '\\' && last != '/') {
            rest.push(SEPARATOR);
        }
        rest.push_str(&other_rest);
    }
    if !rest.is_empty()
        && root.is_empty()
        && drive
            .chars()
            .next_back()
            .is_some_and(|last| !matches!(last, ':' | '\\' | '/'))
    {
        return format!("{drive}{SEPARATOR}{rest}");
    }
    format!("{drive}{root}{rest}")
}

/// `ntpath.expanduser("~")`, or `None` where the `~` stays in place: `USERPROFILE` (even empty), else `HOMEDRIVE`
/// joined with `HOMEPATH`, and nothing without `HOMEPATH`. There is no passwd fallback.
pub(super) fn expanduser(profile: Option<String>, drive: Option<String>, path: Option<String>) -> Option<String> {
    if profile.is_some() {
        return profile;
    }
    Some(join(&drive.unwrap_or_default(), &path?))
}

/// `PureWindowsPath._parse_path`: the drive, the root and the tail parts.
fn parse(path: &str) -> (String, String, Vec<String>) {
    if path.is_empty() {
        return (String::new(), String::new(), Vec::new());
    }
    let (drive, mut root, rest) = splitroot(&path.replace('/', "\\"));
    if root.is_empty() && drive.starts_with(SEPARATOR) && !drive.ends_with(SEPARATOR) {
        let drive_parts: Vec<&str> = drive.split(SEPARATOR).collect();
        if (drive_parts.len() == 4 && !"?.".contains(drive_parts[2])) || drive_parts.len() == 6 {
            root = SEPARATOR.to_string();
        }
    }
    let tail = rest
        .split(SEPARATOR)
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    (drive, root, tail)
}

/// `PureWindowsPath._format_parsed_parts`.
fn format(drive: &str, root: &str, mut tail: Vec<String>) -> String {
    if drive.is_empty() && root.is_empty() && tail.first().is_some_and(|first| !splitroot(first).0.is_empty()) {
        tail.insert(0, ".".to_owned());
    }
    format!("{drive}{root}{}", tail.join("\\"))
}

/// What `str(PureWindowsPath(path))` is before the `or '.'` that `__str__` adds for an empty path.
pub(super) fn normalize(path: &str) -> String {
    let (drive, root, tail) = parse(path);
    format(&drive, &root, tail)
}

/// `str(PureWindowsPath(home) / segment)`.
pub(super) fn path_join(home: &str, segment: &str) -> String {
    let joined = normalize(&join(home, segment));
    if joined.is_empty() { ".".to_owned() } else { joined }
}
