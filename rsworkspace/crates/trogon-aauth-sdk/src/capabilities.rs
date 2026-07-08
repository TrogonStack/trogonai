//! `AAuth-Capabilities` request header helper, per draft "Protocol Primitives"
//! / "AAuth-Capabilities Request Header".
//!
//! Agents SHOULD send this header on signed requests to resources (PS
//! endpoints use the `capabilities` field on `TokenRequest` instead, which
//! already exists on that type). This is a thin wrapper around
//! `Capabilities::to_header_value` so the reqwest driver in `exchange` can
//! attach it in one call.

use trogon_identity_types::aauth::headers::{self, Capabilities, Capability};

/// Extension trait attaching an `AAuth-Capabilities` header to an outbound
/// `reqwest::RequestBuilder`.
pub trait CapabilitiesHeaderExt {
    #[must_use]
    fn aauth_capabilities(self, caps: &[Capability]) -> Self;
}

impl CapabilitiesHeaderExt for reqwest::RequestBuilder {
    fn aauth_capabilities(self, caps: &[Capability]) -> Self {
        let value = Capabilities(caps.to_vec()).to_header_value();
        self.header(headers::CAPABILITIES, value)
    }
}

/// Returns the `(header name, header value)` pair for `AAuth-Capabilities`,
/// for callers driving a transport other than `reqwest`.
#[must_use]
pub fn capabilities_header(caps: &[Capability]) -> (&'static str, String) {
    (headers::CAPABILITIES, Capabilities(caps.to_vec()).to_header_value())
}

#[cfg(test)]
mod tests;
