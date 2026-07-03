//! HTTP / NATS header names used by the AAuth wire protocol.

pub const REQUIREMENT: &str = "AAuth-Requirement";
pub const ACCESS: &str = "AAuth-Access";
pub const MISSION: &str = "AAuth-Mission";
pub const CAPABILITIES: &str = "AAuth-Capabilities";

// RFC 9421 HTTP path
pub const SIGNATURE_KEY: &str = "Signature-Key";
pub const SIGNATURE_INPUT: &str = "Signature-Input";
pub const SIGNATURE: &str = "Signature";
pub const CONTENT_DIGEST: &str = "Content-Digest";

// NATS path (Trogon-defined, mirrors RFC 9421 shape).
pub const NATS_TOKEN: &str = "AAuth-Token";
pub const NATS_SIG_INPUT: &str = "AAuth-Sig-Input";
pub const NATS_SIG: &str = "AAuth-Sig";
pub const NATS_SIG_CREATED: &str = "AAuth-Sig-Created";
pub const NATS_SIG_NONCE: &str = "AAuth-Sig-Nonce";
