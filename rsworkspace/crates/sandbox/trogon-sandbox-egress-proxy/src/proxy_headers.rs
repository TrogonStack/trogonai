use std::borrow::Cow;

mod constants;
use constants::PROXY_HOP_HEADERS;

pub(crate) fn is_proxy_hop_header(name: &str) -> bool {
    PROXY_HOP_HEADERS.iter().any(|hop| name.eq_ignore_ascii_case(hop))
}

/// The headers of a request without the ones addressed to this proxy, so the
/// lease token never reaches a filter, a callback, a mutator or the upstream.
pub(crate) fn without_proxy_headers(headers: &[(String, String)]) -> Cow<'_, [(String, String)]> {
    if headers.iter().any(|(name, _)| is_proxy_hop_header(name)) {
        Cow::Owned(
            headers
                .iter()
                .filter(|(name, _)| !is_proxy_hop_header(name))
                .cloned()
                .collect(),
        )
    } else {
        Cow::Borrowed(headers)
    }
}
