use std::io;

use crate::denial::Denial;
use crate::tls_intercept_error::TlsInterceptError;

/// Outcome of handling one client connection, useful for callers that want to
/// log/metric the result without this crate depending on any tracing/metrics
/// backend choice.
#[derive(Debug)]
pub enum ProxyOutcome {
    /// Connection was allowed, spliced, and the splice finished (in either
    /// direction reaching EOF, or an I/O error occurred after the splice
    /// started).
    Completed {
        bytes_client_to_upstream: u64,
        bytes_upstream_to_client: u64,
    },
    /// Connection was denied by policy before any upstream connection was
    /// attempted.
    Denied(Denial),
    /// Connection was allowed by policy, but connecting to the upstream
    /// destination failed.
    UpstreamConnectFailed(io::Error),
    /// An I/O error occurred while sniffing the client's initial bytes.
    SniffFailed(io::Error),
    /// A Postgres `SSLRequest` was detected, but connecting to upstream (to
    /// relay the negotiation before any policy decision can be made) failed.
    PostgresUpstreamConnectFailed(io::Error),
    /// A Postgres `SSLRequest` was detected, but relaying the negotiation
    /// (the `SSLRequest` itself, upstream's `S`/`N`/`E` reply, or the
    /// subsequent ClientHello) failed.
    PostgresNegotiationFailed(io::Error),
    /// A domain rule matched with a `Transform` or `Forward` action, but no
    /// SNI was available to terminate TLS against. This should not happen in
    /// practice (a domain rule can only match via SNI), but is handled as an
    /// explicit error path rather than a panic or a silent fallback to raw
    /// splice.
    TlsInterceptRequiresSni,
    /// A domain rule matched with a `Transform` or `Forward` action, but
    /// terminating TLS with the client and/or re-encrypting to upstream
    /// failed.
    TlsInterceptFailed(TlsInterceptError),
}
