fn tokenize_json_path(path: &str) -> Vec<String> {
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
                for inner in chars.by_ref() {
                    if inner == ']' {
                        break;
                    }
                    index.push(inner);
                }
                if !index.is_empty() {
                    tokens.push(index);
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
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
        let tokens = tokenize_json_path(stripped);
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
