/// Keeps a failing clone's stderr from echoing the credential back into an evidence stream.
///
/// git prints the remote URL on failure, and that URL is the one we built with the token in
/// it. The unauthenticated URL is a safe substitute because it is what the tenant sent us.
pub(super) fn redact(stderr: &str, original_url: &str) -> String {
    let Some(at) = original_url.find("://").map(|i| i + 3) else {
        return stderr.to_string();
    };
    let (scheme, host_and_path) = original_url.split_at(at);
    let mut cleaned = String::with_capacity(stderr.len());
    for line in stderr.lines() {
        // Any occurrence of `<scheme>...@<host_and_path>` is a credentialed form of the
        // tenant's own URL, so replace the whole span with the bare URL.
        cleaned.push_str(&replace_credentialed(line, scheme, host_and_path));
        cleaned.push('\n');
    }
    cleaned.trim_end().to_string()
}

fn replace_credentialed(line: &str, scheme: &str, host_and_path: &str) -> String {
    let bare = format!("{scheme}{host_and_path}");
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find(scheme) {
        out.push_str(&rest[..start]);
        let after_scheme = &rest[start + scheme.len()..];
        match after_scheme.find(host_and_path) {
            Some(end) if after_scheme[..end].ends_with('@') || end == 0 => {
                out.push_str(&bare);
                rest = &after_scheme[end + host_and_path.len()..];
            }
            _ => {
                out.push_str(scheme);
                rest = after_scheme;
            }
        }
    }
    out.push_str(rest);
    out
}
