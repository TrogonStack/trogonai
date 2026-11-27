use trogon_sandbox_contract::ResponseHeaderStrip;

/// The folded form of a raw response header name: lower case, with `_` and `.` read as `-`.
///
/// It is the form a [`StrippedHeader`](trogon_sandbox_contract::StrippedHeader) is kept in, so a raw name compares
/// against it directly.
pub fn fold_header_name(raw: &[u8]) -> String {
    raw.iter()
        .map(|byte| match byte {
            b'_' | b'.' => '-',
            other => other.to_ascii_lowercase() as char,
        })
        .collect()
}

/// Reports whether the response header `header_name`, as it arrived on the wire, is one of the headers `strip`
/// removes.
pub fn response_header_is_stripped(strip: &ResponseHeaderStrip, header_name: &[u8]) -> bool {
    if strip.is_empty() {
        return false;
    }
    let folded = fold_header_name(header_name);
    strip.headers().iter().any(|header| header.as_str() == folded)
}
