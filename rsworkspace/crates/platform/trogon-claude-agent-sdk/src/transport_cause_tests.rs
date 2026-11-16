use std::error::Error as StdError;
use std::io;

use super::{TransportCause, transport_cause};
use crate::adapters::NameLookupError;

/// An error that only points at the next one, as the transport's own layers do.
#[derive(Debug, thiserror::Error)]
#[error("layer")]
struct LayerError(#[source] Box<dyn StdError + Send + Sync>);

fn layered(inner: impl StdError + Send + Sync + 'static) -> LayerError {
    LayerError(Box::new(LayerError(Box::new(inner))))
}

fn lookup_failure() -> NameLookupError {
    NameLookupError::new(io::Error::other("failed to resolve"))
}

#[test]
fn a_name_lookup_error_anywhere_in_the_chain_is_a_missing_host() {
    assert_eq!(transport_cause(&lookup_failure()), Some(TransportCause::NameNotFound));
    assert_eq!(
        transport_cause(&layered(lookup_failure())),
        Some(TransportCause::NameNotFound)
    );
}

#[test]
fn a_name_lookup_error_wrapped_in_an_io_error_is_a_missing_host() {
    let wrapped = io::Error::other(lookup_failure());
    assert_eq!(transport_cause(&layered(wrapped)), Some(TransportCause::NameNotFound));
    let twice = io::Error::other(io::Error::other(lookup_failure()));
    assert_eq!(transport_cause(&twice), Some(TransportCause::NameNotFound));
}

#[test]
fn a_name_lookup_error_is_read_before_the_io_kind_it_carries() {
    let refused = NameLookupError::new(io::Error::from(io::ErrorKind::ConnectionRefused));
    assert_eq!(transport_cause(&layered(refused)), Some(TransportCause::NameNotFound));
}

#[test]
fn an_io_kind_above_a_name_lookup_error_is_read_first() {
    let reset = io::Error::new(io::ErrorKind::ConnectionReset, layered(lookup_failure()));
    assert_eq!(transport_cause(&reset), Some(TransportCause::Reset));
}

#[test]
fn an_io_kind_wrapping_a_name_lookup_error_directly_is_read_first() {
    let reset = io::Error::new(io::ErrorKind::ConnectionReset, lookup_failure());
    assert_eq!(transport_cause(&reset), Some(TransportCause::Reset));
    assert_eq!(
        transport_cause(&layered(io::Error::new(
            io::ErrorKind::ConnectionReset,
            lookup_failure()
        ))),
        Some(TransportCause::Reset)
    );
    let refused = io::Error::new(io::ErrorKind::ConnectionRefused, lookup_failure());
    assert_eq!(transport_cause(&refused), Some(TransportCause::Refused));
}

#[test]
fn an_io_kind_wrapped_in_another_io_error_is_read() {
    let reset = io::Error::other(io::Error::from(io::ErrorKind::ConnectionReset));
    assert_eq!(transport_cause(&layered(reset)), Some(TransportCause::Reset));
}

#[test]
fn a_refused_connection_is_refused() {
    let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
    assert_eq!(transport_cause(&layered(refused)), Some(TransportCause::Refused));
}

#[test]
fn a_reset_aborted_or_ended_connection_is_reset() {
    for kind in [
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::UnexpectedEof,
    ] {
        assert_eq!(
            transport_cause(&layered(io::Error::from(kind))),
            Some(TransportCause::Reset),
            "{kind:?}"
        );
    }
}

#[test]
fn other_io_kinds_name_no_cause() {
    for kind in [
        io::ErrorKind::NotFound,
        io::ErrorKind::NotConnected,
        io::ErrorKind::TimedOut,
        io::ErrorKind::InvalidData,
        io::ErrorKind::InvalidInput,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::Interrupted,
        io::ErrorKind::Other,
    ] {
        assert_eq!(transport_cause(&layered(io::Error::from(kind))), None, "{kind:?}");
    }
}

#[test]
fn a_plain_io_error_from_a_custom_resolver_names_no_cause() {
    let not_found = io::Error::new(io::ErrorKind::NotFound, "no such host");
    assert_eq!(transport_cause(&layered(not_found)), None);
}
