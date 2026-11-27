//! Stable deny contract for HTTP clients: the status plus an
//! `X-Proxy-Error` header whose value is one of these.

mod constants;

pub use constants::{
    ADDRESS_DENIED, BAD_REQUEST, BAD_UPSTREAM, BLOCKED, NAME, OPAQUE_TUNNEL, TOO_MANY_TUNNELS, UNAUTHENTICATED,
    UNREACHABLE,
};
