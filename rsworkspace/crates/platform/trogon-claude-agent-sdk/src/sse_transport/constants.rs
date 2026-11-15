/// TypeScript `d4`: the text envelope arm of the browser transport is ported.
pub const G4_TEXT_ENVELOPE_ARM_PORTED: bool = true;

/// TypeScript `s4`: lines waiting to be written before a writer has to wait.
pub(super) const QUEUE_LIMIT: usize = 1_000;

/// TypeScript `i4`: failed attempts in a row after which a line is dropped.
pub(super) const UPLOAD_ATTEMPTS: u32 = 3;

/// How much of an error response is read to find its reason.
pub(super) const REASON_BYTES: usize = 4_096;
