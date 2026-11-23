use std::str::FromStr;

use crate::constants::DIGEST_PREFIX;
use crate::{Digest, OciImageRef};

pub(crate) fn split_digest(value: &str) -> (&str, Option<Digest>) {
    match value.rsplit_once('@') {
        Some((before, digest)) if digest.starts_with(DIGEST_PREFIX) => (before, Digest::from_str(digest).ok()),
        _ => (value, None),
    }
}

pub(crate) fn split_tag(value: &str) -> (&str, Option<&str>) {
    if let Some(index) = value.rfind(':') {
        let after = &value[index + 1..];
        if !after.is_empty() && !after.contains('/') {
            return (&value[..index], Some(after));
        }
    }
    (value, None)
}

pub(crate) fn split_registry(name: &str) -> (String, String) {
    match name.find('/') {
        None => (String::new(), name.to_string()),
        Some(index) => {
            let first = &name[..index];
            if first.contains('.') || first.contains(':') || first == "localhost" {
                (first.to_string(), name[index + 1..].to_string())
            } else {
                (String::new(), name.to_string())
            }
        }
    }
}

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
