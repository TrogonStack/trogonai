//! Chain sequence value object.

use std::fmt;

/// The 1-based position of an entry within an audit hash chain.
///
/// This is intentionally a domain concept distinct from the JetStream stream
/// sequence. The two are expected to move together for a stream that carries
/// only chained audit events (see the crate README's threat model), but
/// `ChainSequence` exists so `verify_chain` can reason about gaps and
/// ordering without depending on JetStream types at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChainSequence(u64);

impl ChainSequence {
    /// The sequence of the first entry in a chain.
    pub const FIRST: Self = Self(1);

    /// Creates a `ChainSequence`, rejecting zero because chain sequences are
    /// 1-based (mirrors JetStream's own 1-based stream sequence numbering).
    pub fn try_new(value: u64) -> Result<Self, InvalidChainSequence> {
        if value == 0 {
            return Err(InvalidChainSequence);
        }
        Ok(Self(value))
    }

    /// Returns the next sequence in the chain.
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// Returns the underlying `u64`.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ChainSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Error returned when constructing a [`ChainSequence`] from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("chain sequence must be at least 1")]
pub struct InvalidChainSequence;

#[cfg(test)]
mod tests;
