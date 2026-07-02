//! Observability for gateway interception decisions.
//!
//! Kept intentionally small: `mcp-gateway` has no `mcp.gateway.*` semantic
//! conventions registered in `trogon-semconv` yet, so this module emits
//! `tracing` spans/events with ad-hoc field names rather than inventing new
//! semconv constants outside that crate's ownership. Promoting these to
//! real semconv attributes is a natural follow-up once the gateway ships.

pub mod verdict;
