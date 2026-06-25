//! Wire-level caller-JWT header constants and parser.
//!
//! Re-exports `a2a-auth-callout`'s definitions so downstream consumers of
//! this crate don't need a direct dependency on `a2a-auth-callout` just to
//! reference the header name or parse the header value.

pub use a2a_auth_callout::caller_jwt_header::{CALLER_JWT_HEADER_NAME, CallerJwtHeaderValue};
