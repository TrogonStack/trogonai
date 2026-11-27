/// Collapses `.`, `..` and duplicate slashes of an absolute path.
pub(crate) fn normalize(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    format!("/{}", segments.join("/"))
}

pub(crate) fn file_uri(absolute_path: &str) -> String {
    let encoded: Vec<String> = absolute_path.split('/').map(encode_component).collect();
    format!("file://{}", encoded.join("/"))
}

/// Percent-encodes everything except ASCII letters, digits and `-_.!~*'()`.
fn encode_component(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
