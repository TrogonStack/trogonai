use std::str::FromStr;

use crate::Digest;

/// Splits a trailing `@digest` off an image reference, only when it parses as a [`Digest`].
///
/// An invalid suffix stays part of the reference, so a malformed digest pattern never widens to the
/// bare repository.
pub(crate) fn split_digest(value: &str) -> (&str, Option<Digest>) {
    value
        .rsplit_once('@')
        .and_then(|(before, digest)| Digest::from_str(digest).ok().map(|digest| (before, Some(digest))))
        .unwrap_or((value, None))
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
