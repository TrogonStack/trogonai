//! Crate-wide constants.

/// RFC 7517 registered media type for a JWK Set. Preferred over the generic
/// `application/json` because it lets clients identify the payload shape
/// from `Content-Type` alone without sniffing the body.
pub(crate) const JWK_SET_CONTENT_TYPE: &str = "application/jwk-set+json";
