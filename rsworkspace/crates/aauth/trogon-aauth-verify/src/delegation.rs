//! Delegation chain (`act` claim) verification per draft "Agent Delegation" /
//! "Delegation Chain".
//!
//! The draft records the upstream delegation chain in an auth token's `act`
//! claim: `act.agent` names the immediate upstream agent (the intermediary
//! resource in call chaining, or the parent agent in sub-agent authorization),
//! and a nested `act.act` records that upstream's own upstream, recursively.
//! Separately, "Single-Level Depth" caps *sub-agent* nesting specifically (a
//! sub-agent's agent token MUST NOT itself carry a `parent_agent` pointing at
//! another sub-agent) -- that is a distinct rule from how many `act` hops a
//! call-chaining scenario may accumulate. The draft does not state a numeric
//! bound on `act` chain length, so this module enforces its own defensive
//! recursion limit ([`MAX_CHAIN_DEPTH`]) purely to keep a malicious or
//! malformed token from driving unbounded recursion; it is an implementation
//! safety bound, not a normative protocol limit.

use trogon_identity_types::aauth::Act;

/// Defensive recursion bound for `act` chain traversal. Not specified by the
/// draft; chosen generously above any plausible legitimate delegation depth
/// (call chaining hops + at most one sub-agent hop) while still bounding
/// worst-case work for a crafted token.
pub const MAX_CHAIN_DEPTH: usize = 16;

/// Errors verifying an `act` delegation chain.
#[derive(Debug, thiserror::Error)]
pub enum DelegationError {
    /// `act.agent` is not a syntactically valid AAuth agent identifier per
    /// "Agent Identifiers" (`aauth:local@domain`, lowercase local part).
    #[error("act chain entry at depth {depth} has an invalid agent identifier: {agent:?}")]
    InvalidAgentIdentifier { depth: usize, agent: String },
    /// The chain nests deeper than [`MAX_CHAIN_DEPTH`].
    #[error("act chain exceeds the maximum supported depth of {MAX_CHAIN_DEPTH}")]
    TooDeep,
}

/// One flattened entry from an `act` chain, in upstream order (immediate
/// presenter's delegator first, root delegator last). Produced by
/// [`flatten_act_chain`] for audit logging or policy evaluation that wants a
/// linear view instead of the recursive `Act` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenedActEntry {
    /// Zero-based depth: 0 is the immediate upstream (`act.agent`), 1 is that
    /// agent's upstream (`act.act.agent`), and so on.
    pub depth: usize,
    pub agent: String,
}

/// Validates an `act` delegation chain: every `agent` value must be a
/// syntactically valid AAuth agent identifier (#agent-identifiers), and the
/// chain must not exceed [`MAX_CHAIN_DEPTH`] nested levels.
///
/// This checks shape only -- it does not verify that each named agent
/// actually authorized the delegation (that requires the signing context at
/// each hop, which the draft's "Upstream Token Verification" flow supplies
/// hop-by-hop; see [`crate::upstream::verify_upstream_token`]). Callers that
/// need to confirm rule 8 of "Request-Context Binding" (`act` "accurately
/// reflects the upstream delegation context") pair this with an explicit
/// comparison against the expected upstream agent identifier at the call
/// site, since the identifier a resource expects is only known there.
pub fn verify_act_chain(act: &Act) -> Result<(), DelegationError> {
    let mut current = act;
    let mut depth = 0usize;
    loop {
        if !is_valid_agent_identifier(&current.agent) {
            return Err(DelegationError::InvalidAgentIdentifier {
                depth,
                agent: current.agent.clone(),
            });
        }
        match &current.act {
            None => return Ok(()),
            Some(next) => {
                depth += 1;
                if depth >= MAX_CHAIN_DEPTH {
                    return Err(DelegationError::TooDeep);
                }
                current = next;
            }
        }
    }
}

/// Flattens an `act` chain into upstream-ordered entries for audit logging.
/// Does not validate identifier shape -- callers that need validated data
/// should call [`verify_act_chain`] first.
#[must_use]
pub fn flatten_act_chain(act: &Act) -> Vec<FlattenedActEntry> {
    let mut out = Vec::new();
    let mut current = act;
    let mut depth = 0usize;
    loop {
        out.push(FlattenedActEntry {
            depth,
            agent: current.agent.clone(),
        });
        match &current.act {
            None => return out,
            Some(next) => {
                depth += 1;
                if depth >= MAX_CHAIN_DEPTH {
                    return out;
                }
                current = next;
            }
        }
    }
}

/// Validates an AAuth agent identifier per "Agent Identifiers":
/// `aauth:local@domain` where `local` is lowercase ASCII letters, digits,
/// `-`, `_`, `+`, `.` (non-empty, at most 255 bytes) and `domain` is a
/// non-empty host string containing no `://` scheme separator.
///
/// This is a syntactic check only; it does not perform DNS validation of
/// `domain` or apply the Server Identifier scheme/port/path restrictions,
/// which are out of scope for validating an `act` entry.
#[must_use]
pub fn is_valid_agent_identifier(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("aauth:") else {
        return false;
    };
    let Some((local, domain)) = rest.split_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 255 {
        return false;
    }
    if !local
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'+' | b'.'))
    {
        return false;
    }
    if domain.is_empty() || domain.contains("://") {
        return false;
    }
    true
}

#[cfg(test)]
mod tests;
