/// Tokenize a `$.`-prefixed JSONPath (with the leading `$.` already
/// stripped) into reference tokens. Returns `None` when the input
/// contains a malformed segment that would silently drop information
/// — an empty bracket `[]`, an unterminated `[` (no closing `]`), or
/// trailing whitespace — so callers fail-closed instead of mapping
/// `$.parts[].text` to `parts/text` (which silently elides the
/// intended array indexing and lets sensitive data through).
fn tokenize_json_path(path: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '[' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
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
                if !closed || index.is_empty() {
                    return None;
                }
                tokens.push(index);
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Some(tokens)
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
