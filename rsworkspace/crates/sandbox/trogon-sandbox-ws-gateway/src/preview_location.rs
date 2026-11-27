use axum::http::HeaderValue;

/// Rewrites a `Location` header from the upstream so a redirect stays under the preview prefix. Redirects to other
/// origins, and values that are not URLs, are left as they are.
pub(crate) fn preview_location(
    location: &HeaderValue,
    target: &reqwest::Url,
    base_path: &str,
    prefix: &str,
) -> HeaderValue {
    let Some(destination) = location.to_str().ok().and_then(|value| target.join(value).ok()) else {
        return location.clone();
    };
    if destination.origin() != target.origin() {
        return location.clone();
    }
    let path = destination.path();
    let path = match path.strip_prefix(base_path) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
        _ => path,
    };
    let mut rewritten = format!("{prefix}{}", if path.is_empty() { "/" } else { path });
    if let Some(query) = destination.query() {
        rewritten.push('?');
        rewritten.push_str(query);
    }
    if let Some(fragment) = destination.fragment() {
        rewritten.push('#');
        rewritten.push_str(fragment);
    }
    HeaderValue::from_str(&rewritten).unwrap_or_else(|_| location.clone())
}

#[cfg(test)]
mod tests;
