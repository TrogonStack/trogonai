/// Tokenize a `$.`-prefixed JSONPath (with the leading `$.` already
/// stripped) into reference tokens. Returns `None` when the input
/// contains a malformed segment that would silently drop information
/// — an empty bracket `[]`, an unterminated `[` (no closing `]`),
/// padded brackets like `[ 0 ]`, or a token with leading/trailing
/// whitespace — so callers fail-closed instead of mapping
/// `$.parts[].text` or `$. params.x` to a surprising pointer (which
/// silently elides the intended index or shifts the key, letting
/// sensitive data through unredacted).
fn tokenize_json_path(path: &str) -> Option<Vec<String>> {
    // A leading `.` (from `$..foo`), any `..` mid-path, or a
    // trailing `.` are all malformed: they would silently elide
    // segments or retarget the path to a sibling key. Reject up
    // front so the structural checks below only deal with internal
    // composition rules.
    if path.starts_with('.') || path.ends_with('.') || path.contains("..") {
        return None;
    }
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = path.chars().peekable();
    // True immediately after a `]`. The next char must be a
    // delimiter (`.` or `[`); bare text would silently re-target
    // the path (e.g. `$.parts[0]text` previously normalized to
    // `/parts/0/text` instead of failing).
    let mut just_closed_bracket = false;
    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if just_closed_bracket {
                    just_closed_bracket = false;
                    continue;
                }
                if current.is_empty() {
                    // `..` is rejected up front; an unrelated empty
                    // segment here would be a tokenizer bug.
                    return None;
                }
                let segment = std::mem::take(&mut current);
                if !is_valid_segment(&segment) {
                    return None;
                }
                tokens.push(segment);
            }
            '[' => {
                if !current.is_empty() {
                    let segment = std::mem::take(&mut current);
                    if !is_valid_segment(&segment) {
                        return None;
                    }
                    tokens.push(segment);
                }
                let mut index = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == ']' {
                        closed = true;
                        break;
                    }
                    index.push(inner);
                }
                if !closed || index.is_empty() || !is_valid_segment(&index) {
                    return None;
                }
                tokens.push(index);
                just_closed_bracket = true;
            }
            _ => {
                if just_closed_bracket {
                    // `parts[0]text` — bare text immediately after a
                    // closed bracket would otherwise be appended as a
                    // new segment without an intervening `.` and
                    // produce a surprising pointer.
                    return None;
                }
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        if !is_valid_segment(&current) {
            return None;
        }
        tokens.push(current);
    }
    Some(tokens)
}

/// A segment is valid only when it has no surrounding whitespace.
/// Internal whitespace is allowed because some JSON keys legitimately
/// contain spaces, but a token like `" params"` or `" 0 "` almost
/// always means the manifest author intended `params` / `0` and the
/// padding is operator error. Failing closed surfaces the typo at
/// config-load time instead of silently no-opping at runtime.
fn is_valid_segment(segment: &str) -> bool {
    segment == segment.trim()
}

pub fn to_json_pointer(path: &str) -> Option<String> {
    // The canonical JSONPath root is `$` — without this branch a caller
    // passing the literal root path would get `None` and miss the
    // whole document.
    if path == "$" {
        return Some(String::new());
    }
    if path.starts_with('/') {
        return Some(path.to_owned());
    }
    if let Some(stripped) = path.strip_prefix("$.") {
        if stripped.is_empty() {
            return Some(String::new());
        }
        let tokens = tokenize_json_path(stripped)?;
        if tokens.is_empty() {
            return None;
        }
        // Per RFC 6901, `~` and `/` inside a token must be escaped as
        // `~0` and `~1` respectively. Without these escapes, a JSONPath
        // token containing a slash would split unexpectedly when fed to
        // `serde_json::Value::pointer`.
        let pointer = tokens.iter().fold(String::from("/"), |acc, segment| {
            let escaped = segment.replace('~', "~0").replace('/', "~1");
            format!("{acc}{escaped}/")
        });
        return Some(pointer.trim_end_matches('/').to_owned());
    }
    None
}

pub fn value_at_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let pointer = to_json_pointer(path)?;
    root.pointer(&pointer)
}

#[cfg(test)]
mod tests;
