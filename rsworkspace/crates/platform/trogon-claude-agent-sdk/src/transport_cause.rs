//! Reads what a failed request ran into from the error's source chain, by type.
//!
//! The walk visits the chain from the outermost error inwards and stops at the first error that names a cause. At
//! each level an `io::Error` also shows the error it wraps, since `io::Error` writes the wrapped error's text as its
//! own while its `source` skips it; the `io::Error`'s own kind is read before the error it wraps, as any outer error
//! is read before an inner one.

use std::error::Error as StdError;
use std::io;

use crate::adapters::NameLookupError;

/// What a request that got no answer ran into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransportCause {
    /// The resolver found no address for the host ([`NameLookupError`]).
    NameNotFound,
    /// The connection closed before the answer was complete (hyper's incomplete message).
    ClosedEarly,
    /// The peer refused the connection.
    Refused,
    /// The connection was reset, aborted or ended mid-read.
    Reset,
}

pub(crate) fn transport_cause(error: &(dyn StdError + 'static)) -> Option<TransportCause> {
    let mut source = Some(error);
    while let Some(current) = source {
        if let Some(cause) = shown(current).find_map(named_cause) {
            return Some(cause);
        }
        source = current.source();
    }
    None
}

/// The cause one error names by its own type or `io::Error` kind, before anything it wraps is read.
fn named_cause(error: &(dyn StdError + 'static)) -> Option<TransportCause> {
    if error.is::<NameLookupError>() {
        return Some(TransportCause::NameNotFound);
    }
    if incomplete_message(error) {
        return Some(TransportCause::ClosedEarly);
    }
    match error.downcast_ref::<io::Error>()?.kind() {
        io::ErrorKind::ConnectionRefused => Some(TransportCause::Refused),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::UnexpectedEof => {
            Some(TransportCause::Reset)
        }
        _ => None,
    }
}

/// The error itself and, through any `io::Error` that wraps another error, each wrapped error.
fn shown<'a>(error: &'a (dyn StdError + 'static)) -> impl Iterator<Item = &'a (dyn StdError + 'static)> {
    std::iter::successors(Some(error), |current| {
        let current: &'a (dyn StdError + 'static) = *current;
        current
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|wrapped| wrapped as &'a (dyn StdError + 'static))
    })
}

fn incomplete_message(error: &(dyn StdError + 'static)) -> bool {
    error
        .downcast_ref::<hyper::Error>()
        .is_some_and(hyper::Error::is_incomplete_message)
}

#[cfg(test)]
#[path = "transport_cause_tests.rs"]
mod tests;
