//! A hook that edits the headers of a request in place just before it goes
//! upstream, on a terminated or a plain-HTTP request.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use trogon_sandbox_contract::Host;

use crate::header_syntax::{fold_header_name, is_header_value, is_token};
use crate::proxy_headers::without_proxy_headers;

mod constants;
use constants::FRAMING_HEADERS;

/// Edits the headers that will be sent upstream, in place. It runs after the
/// request was allowed and its decision's header edits applied, with the
/// destination the allowlist evaluated, never the client's `Host` header.
pub trait HeaderMutator: Send + Sync {
    fn mutate(&self, headers: &mut Vec<(String, String)>, destination: &Host);
}

impl<F> HeaderMutator for F
where
    F: Fn(&mut Vec<(String, String)>, &Host) + Send + Sync,
{
    fn mutate(&self, headers: &mut Vec<(String, String)>, destination: &Host) {
        self(headers, destination)
    }
}

/// A mutator whose result cannot be forwarded: it panicked, produced a header
/// that is not well formed, or touched a header that frames the request.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("the header mutator produced headers that cannot be forwarded")]
pub struct MutationRefusedError;

/// Runs `mutator` on a copy of `headers` and returns the result.
pub(crate) fn mutated(
    mutator: &Arc<dyn HeaderMutator>,
    headers: &[(String, String)],
    destination: &Host,
) -> Result<Vec<(String, String)>, MutationRefusedError> {
    let headers = without_proxy_headers(headers);
    let headers = headers.as_ref();
    let mut edited = headers.to_vec();
    catch_unwind(AssertUnwindSafe(|| mutator.mutate(&mut edited, destination))).map_err(|_| MutationRefusedError)?;
    let well_formed = edited
        .iter()
        .all(|(name, value)| is_token(name) && is_header_value(value));
    if !well_formed || framing(headers) != framing(&edited) {
        return Err(MutationRefusedError);
    }
    Ok(edited)
}

fn framing(headers: &[(String, String)]) -> Vec<(String, &str)> {
    headers
        .iter()
        .map(|(name, value)| (fold_header_name(name), value.as_str()))
        .filter(|(name, _)| FRAMING_HEADERS.contains(&name.as_str()))
        .collect()
}

#[cfg(test)]
mod tests;
