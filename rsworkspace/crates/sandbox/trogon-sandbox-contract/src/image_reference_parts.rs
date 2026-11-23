use std::str::FromStr;

use crate::Digest;
use crate::constants::DIGEST_PREFIX;

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
