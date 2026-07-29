//! Crate-wide constants.

/// Header carrying the agent's presentation of its `aa-agent+jwt`, per
/// "Agent Token Request": `Signature-Key: sig=jwt;jwt="<token>"`. Only the
/// embedded token is extracted here; see module docs for the signature
/// verification gap.
pub(crate) const SIGNATURE_KEY_HEADER: &str = "signature-key";

/// Maximum clarification round-trips per pending request, per "Clarification
/// Limits": "PSes SHOULD enforce a maximum number of clarification rounds
/// (e.g., 5) to prevent indefinite chat loops."
pub const MAX_CLARIFICATION_ROUNDS: u32 = 5;
