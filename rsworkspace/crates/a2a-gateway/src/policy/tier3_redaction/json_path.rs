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
        let pointer = tokens
            .iter()
            .fold(String::from("/"), |acc, segment| format!("{acc}{segment}/"));
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
