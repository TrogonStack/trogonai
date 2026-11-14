//! Node's `path.win32.join` and `path.win32.normalize`, ported over UTF-16 code units the way the JavaScript
//! indexes strings, so every length, slice and index means what it means upstream.

const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;
const COLON: u16 = b':' as u16;
const DOT: u16 = b'.' as u16;
const QUESTION: u16 = b'?' as u16;

const RESERVED_NAMES: [&str; 28] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn is_separator(code: u16) -> bool {
    code == SLASH || code == BACKSLASH
}

fn is_device_root(code: u16) -> bool {
    u8::try_from(code).is_ok_and(|byte| byte.is_ascii_alphabetic())
}

/// `String.prototype.slice(start, end)` for the non-negative or negative indexes the algorithm uses.
fn slice(path: &[u16], start: isize, end: isize) -> &[u16] {
    let length = isize::try_from(path.len()).unwrap_or(isize::MAX);
    let resolve = |index: isize| {
        if index < 0 {
            (length + index).max(0)
        } else {
            index.min(length)
        }
    };
    let (from, to) = (resolve(start), resolve(end));
    if from >= to {
        return &[];
    }
    &path[from as usize..to as usize]
}

/// `String.prototype.indexOf(character, from)`, `-1` when absent.
fn index_of(path: &[u16], needle: u16, from: usize) -> isize {
    path.iter()
        .enumerate()
        .skip(from)
        .find(|(_, code)| **code == needle)
        .map_or(-1, |(index, _)| index as isize)
}

fn is_reserved_name(path: &[u16], colon_index: isize) -> bool {
    let device = text(slice(path, 0, colon_index)).to_uppercase();
    RESERVED_NAMES.contains(&device.as_str())
}

/// `path.win32.join(base, segment)`; an empty argument is skipped like Node skips it.
pub(super) fn join(base: &str, segment: &str) -> String {
    let parts: Vec<&str> = [base, segment].into_iter().filter(|part| !part.is_empty()).collect();
    let Some(first) = parts.first() else {
        return ".".to_owned();
    };
    let first = units(first);
    let mut joined = units(&parts.join("\\"));

    let mut needs_replace = true;
    let mut slash_count = 0;
    if first.first().copied().is_some_and(is_separator) {
        slash_count += 1;
        if first.len() > 1 && is_separator(first[1]) {
            slash_count += 1;
            if first.len() > 2 {
                if is_separator(first[2]) {
                    slash_count += 1;
                } else {
                    needs_replace = false;
                }
            }
        }
    }
    if needs_replace {
        while slash_count < joined.len() && is_separator(joined[slash_count]) {
            slash_count += 1;
        }
        if slash_count >= 2 {
            joined.splice(..slash_count, [BACKSLASH]);
        }
    }

    let mut segments: Vec<Vec<u16>> = Vec::new();
    let mut current: Vec<u16> = Vec::new();
    let mut index = 0;
    while index < joined.len() {
        if joined[index] == BACKSLASH {
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            while index + 1 < joined.len() && joined[index + 1] == BACKSLASH {
                index += 1;
            }
        } else {
            current.push(joined[index]);
        }
        index += 1;
    }
    if !current.is_empty() {
        segments.push(current);
    }
    if segments.iter().any(|part| {
        let colon = index_of(part, COLON, 0);
        colon != -1 && is_reserved_name(part, colon)
    }) {
        let swapped: Vec<u16> = joined
            .iter()
            .map(|&code| if code == SLASH { BACKSLASH } else { code })
            .collect();
        return text(&swapped);
    }
    text(&normalize(&joined))
}

/// `path.win32.normalize`.
fn normalize(path: &[u16]) -> Vec<u16> {
    let length = path.len();
    if length == 0 {
        return units(".");
    }
    let code = path[0];
    if length == 1 {
        return if code == SLASH { vec![BACKSLASH] } else { path.to_vec() };
    }
    let mut root_end = 0;
    let mut device: Option<Vec<u16>> = None;
    let mut absolute = false;
    if is_separator(code) {
        absolute = true;
        if is_separator(path[1]) {
            let mut j = 2;
            let mut last = j;
            while j < length && !is_separator(path[j]) {
                j += 1;
            }
            if j < length && j != last {
                let first_part = &path[last..j];
                last = j;
                while j < length && is_separator(path[j]) {
                    j += 1;
                }
                if j < length && j != last {
                    last = j;
                    while j < length && !is_separator(path[j]) {
                        j += 1;
                    }
                    if j == length || j != last {
                        if first_part == [DOT] || first_part == [QUESTION] {
                            let mut root = vec![BACKSLASH, BACKSLASH];
                            root.extend_from_slice(first_part);
                            device = Some(root);
                            root_end = 4;
                            let colon = index_of(path, COLON, 0);
                            let possible = slice(path, 4, colon + 1);
                            if is_reserved_name(possible, possible.len() as isize - 1) {
                                let mut root = units("\\\\?\\");
                                root.extend_from_slice(possible);
                                device = Some(root);
                                root_end = 4 + possible.len();
                            }
                        } else if j == length {
                            let mut root = vec![BACKSLASH, BACKSLASH];
                            root.extend_from_slice(first_part);
                            root.push(BACKSLASH);
                            root.extend_from_slice(&path[last..]);
                            root.push(BACKSLASH);
                            return root;
                        } else {
                            let mut root = vec![BACKSLASH, BACKSLASH];
                            root.extend_from_slice(first_part);
                            root.push(BACKSLASH);
                            root.extend_from_slice(&path[last..j]);
                            device = Some(root);
                            root_end = j;
                        }
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else {
        let colon = index_of(path, COLON, 0);
        if colon > 0 {
            let colon = colon as usize;
            if is_device_root(code) && colon == 1 {
                device = Some(path[..2].to_vec());
                root_end = 2;
                if length > 2 && is_separator(path[2]) {
                    absolute = true;
                    root_end = 3;
                }
            } else if is_reserved_name(path, colon as isize) {
                device = Some(path[..=colon].to_vec());
                root_end = colon + 1;
            }
        }
    }

    let mut tail = if root_end < length {
        normalize_string(&path[root_end..], !absolute)
    } else {
        Vec::new()
    };
    if tail.is_empty() && !absolute {
        tail = units(".");
    }
    if !tail.is_empty() && is_separator(path[length - 1]) {
        tail.push(BACKSLASH);
    }
    let prefixed = |tail: &[u16], device: &[u16]| {
        let mut result = units(".\\");
        result.extend_from_slice(device);
        result.extend_from_slice(tail);
        result
    };
    if !absolute && device.is_none() && path.contains(&COLON) {
        if tail.len() >= 2 && is_device_root(tail[0]) && tail[1] == COLON {
            return prefixed(&tail, &[]);
        }
        let mut index = index_of(path, COLON, 0);
        loop {
            let after = (index + 1) as usize;
            if index == length as isize - 1 || is_separator(path[after]) {
                return prefixed(&tail, &[]);
            }
            index = index_of(path, COLON, after);
            if index == -1 {
                break;
            }
        }
    }
    let colon = index_of(path, COLON, 0);
    if is_reserved_name(path, colon) {
        return prefixed(&tail, device.as_deref().unwrap_or_default());
    }
    let mut result = Vec::new();
    match (&device, absolute) {
        (None, true) => result.push(BACKSLASH),
        (None, false) => {}
        (Some(device), true) => {
            result.extend_from_slice(device);
            result.push(BACKSLASH);
        }
        (Some(device), false) => result.extend_from_slice(device),
    }
    result.extend_from_slice(&tail);
    result
}

/// Node's `normalizeString` with `\` as the separator: resolves `.` and `..` segments.
fn normalize_string(path: &[u16], allow_above_root: bool) -> Vec<u16> {
    let mut result: Vec<u16> = Vec::new();
    let mut last_segment_length: isize = 0;
    let mut last_slash: isize = -1;
    let mut dots: isize = 0;
    let mut code: u16 = 0;
    for i in 0..=path.len() as isize {
        if (i as usize) < path.len() {
            code = path[i as usize];
        } else if is_separator(code) {
            break;
        } else {
            code = SLASH;
        }
        if is_separator(code) {
            if last_slash == i - 1 || dots == 1 {
                // Nothing to add for an empty or `.` segment.
            } else if dots == 2 {
                let length = result.len();
                let above =
                    length < 2 || last_segment_length != 2 || result[length - 1] != DOT || result[length - 2] != DOT;
                if above {
                    if length > 2 {
                        let last_slash_index = length as isize - last_segment_length - 1;
                        if last_slash_index == -1 {
                            result.clear();
                            last_segment_length = 0;
                        } else {
                            result.truncate(last_slash_index as usize);
                            let last_separator = result
                                .iter()
                                .rposition(|&unit| unit == BACKSLASH)
                                .map_or(-1, |index| index as isize);
                            last_segment_length = result.len() as isize - 1 - last_separator;
                        }
                        last_slash = i;
                        dots = 0;
                        continue;
                    } else if length != 0 {
                        result.clear();
                        last_segment_length = 0;
                        last_slash = i;
                        dots = 0;
                        continue;
                    }
                }
                if allow_above_root {
                    if !result.is_empty() {
                        result.push(BACKSLASH);
                    }
                    result.extend_from_slice(&[DOT, DOT]);
                    last_segment_length = 2;
                }
            } else {
                let segment = &path[(last_slash + 1) as usize..i as usize];
                if !result.is_empty() {
                    result.push(BACKSLASH);
                }
                result.extend_from_slice(segment);
                last_segment_length = i - last_slash - 1;
            }
            last_slash = i;
            dots = 0;
        } else if code == DOT && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
    }
    result
}
