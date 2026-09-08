//! Crate-wide constants: AAuth wire values, HTTP/NATS header names, and
//! act-chain bounds.

/// Maximum number of entries allowed in an `act` delegation chain.
pub const MAX_ACT_CHAIN_DEPTH: usize = 8;

/// JWK members that carry private key material, across every key type AAuth
/// can encounter: `d` for EC (RFC 7518 Section 6.2.2), RSA (Section 6.3.2),
/// and OKP (RFC 8037 Section 2); the remaining RSA CRT parameters; and `k`,
/// which *is* the secret for a symmetric `oct` key (Section 6.4.1).
pub const JWK_PRIVATE_MEMBERS: [&str; 8] = ["d", "p", "q", "dp", "dq", "qi", "oth", "k"];

/// JWK members that describe a key without being any part of one: they name
/// which key is meant and what it is for, and none of them is key material of
/// either half. This is an allow-list rather than the inverse of
/// [`JWK_PRIVATE_MEMBERS`] because a member this crate has never heard of must
/// stay unprinted by default.
pub const JWK_DESCRIPTIVE_MEMBERS: [&str; 5] = ["kty", "crv", "kid", "alg", "use"];

/// `kty` value for a symmetric key. Never valid in a confirmation claim: a
/// proof-of-possession key that both parties must hold is not a proof of
/// possession, and publishing it in a token discloses it.
pub const KTY_OCT: &str = "oct";

/// `kty` values an issuer may put in a confirmation claim.
pub const KTY_EC: &str = "EC";
pub const KTY_RSA: &str = "RSA";
pub const KTY_OKP: &str = "OKP";

/// Public members each key type requires before the JWK can verify anything.
/// EC per RFC 7518 Section 6.2.1, RSA per Section 6.3.1, OKP per RFC 8037
/// Section 2. A confirmation key missing any of these is syntactically a JWK
/// but cannot be used to check a proof of possession.
pub const JWK_REQUIRED_EC_MEMBERS: [&str; 3] = ["crv", "x", "y"];
pub const JWK_REQUIRED_RSA_MEMBERS: [&str; 2] = ["n", "e"];
pub const JWK_REQUIRED_OKP_MEMBERS: [&str; 2] = ["crv", "x"];

/// `typ` header value identifying an agent identity token.
pub const TYP_AGENT: &str = "aa-agent+jwt";
/// `typ` header value identifying a resource challenge token.
pub const TYP_RESOURCE: &str = "aa-resource+jwt";
/// `typ` header value identifying an authorization token from a Person Server.
pub const TYP_AUTH: &str = "aa-auth+jwt";

/// `dwk` (discoverable-well-known) values used by AAuth issuers.
pub const DWK_AGENT: &str = "aauth-agent.json";
pub const DWK_RESOURCE: &str = "aauth-resource.json";
pub const DWK_PERSON: &str = "aauth-person.json";
/// `dwk` value for an Access Server, per "Auth Token Structure" / "Access Server Metadata".
pub const DWK_ACCESS: &str = "aauth-access.json";

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
pub const NATS_AUTH_TOKEN: &str = "AAuth-Auth-Token";
