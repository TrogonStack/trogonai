use crate::OciImageRef;

/// Whether an allowlist pattern admits an image.
///
/// The pattern first matches the image's [`OciImageRef::allowlist_string`] the way
/// [`pattern_matches_legacy`] does. Otherwise it is parsed as an image reference and compared by
/// part: an empty registry matches any registry, a trailing `*` on the repository matches a
/// prefix, an absent tag matches any tag, and a `*` tag matches any tag or none.
pub fn pattern_matches_oci_image_ref(pattern: &str, image: &OciImageRef) -> bool {
    if pattern_matches_legacy(pattern, &image.allowlist_string()) {
        return true;
    }
    oci_image_ref_matches(&OciImageRef::parse_unqualified(pattern), image)
}

/// Whether an allowlist pattern admits an image string.
///
/// An empty pattern matches nothing. A single trailing `*` matches any image starting with the
/// rest of the pattern; any other `*` is literal. Every other pattern must equal the image.
pub fn pattern_matches_legacy(pattern: &str, image: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    prefix_or_exact_matches(pattern, image)
}

fn oci_image_ref_matches(pattern: &OciImageRef, candidate: &OciImageRef) -> bool {
    if !pattern.registry.is_empty() && pattern.registry != candidate.registry {
        return false;
    }
    if !prefix_or_exact_matches(&pattern.repository, &candidate.repository) {
        return false;
    }
    match (&pattern.tag, &candidate.tag) {
        (None, _) => true,
        (Some(pattern), Some(tag)) => pattern == "*" || pattern == tag,
        (Some(pattern), None) => pattern == "*",
    }
}

fn prefix_or_exact_matches(pattern: &str, candidate: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) if !prefix.contains('*') => candidate.starts_with(prefix),
        _ => pattern == candidate,
    }
}
