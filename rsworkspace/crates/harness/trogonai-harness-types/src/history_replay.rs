//! Legacy tool-call bookkeeping while history is projected for replay.

/// One legacy `terminal` tool call discovered in an assistant message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyTerminalCall<'id> {
    /// Which assistant message owned the call.
    pub assistant_index: usize,
    /// The call id the result names.
    pub id: &'id str,
    /// The legacy action parsed from the arguments.
    pub action: &'id str,
    /// Whether the call maps to nested `shell` arguments.
    pub mapped: bool,
}

/// Finds the legacy terminal call a tool result belongs to.
#[must_use]
pub fn find_legacy_terminal_call<'id>(
    calls: &[LegacyTerminalCall<'id>],
    assistant_index: Option<usize>,
    id: &str,
) -> Option<LegacyTerminalCall<'id>> {
    let owner = assistant_index?;
    calls
        .iter()
        .copied()
        .find(|call| call.assistant_index == owner && call.id == id)
}

/// Whether a subagent history call still participates in replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentHistoryDisposition {
    /// The call is still live in the replayed request.
    Current,
    /// The call is summarized rather than replayed.
    Inert,
}

/// One legacy `subagent` tool call discovered in an assistant message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubagentHistoryCall<'id> {
    /// Which assistant message owned the call.
    pub assistant_index: usize,
    /// The call id the result names.
    pub id: &'id str,
    /// The legacy action parsed from the arguments.
    pub action: &'id str,
    /// Whether the call is replayed or summarized.
    pub disposition: SubagentHistoryDisposition,
}

/// Finds the subagent history call a tool result belongs to.
#[must_use]
pub fn find_subagent_history_call<'id>(
    calls: &[SubagentHistoryCall<'id>],
    assistant_index: Option<usize>,
    id: &str,
) -> Option<SubagentHistoryCall<'id>> {
    let owner = assistant_index?;
    calls
        .iter()
        .copied()
        .find(|call| call.assistant_index == owner && call.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_lookup_needs_the_assistant_message_that_issued_the_call() {
        let calls = [LegacyTerminalCall {
            assistant_index: 2,
            id: "t1",
            action: "exec",
            mapped: true,
        }];
        assert_eq!(find_legacy_terminal_call(&calls, Some(2), "t1"), Some(calls[0]));
        assert_eq!(find_legacy_terminal_call(&calls, None, "t1"), None);
        assert_eq!(find_legacy_terminal_call(&calls, Some(2), "missing"), None);
        assert_eq!(find_legacy_terminal_call(&calls, Some(1), "t1"), None);
    }

    #[test]
    fn subagent_lookup_matches_on_owner_and_id() {
        let calls = [SubagentHistoryCall {
            assistant_index: 4,
            id: "s9",
            action: "create",
            disposition: SubagentHistoryDisposition::Inert,
        }];
        assert_eq!(find_subagent_history_call(&calls, Some(4), "s9"), Some(calls[0]));
        assert_eq!(find_subagent_history_call(&calls, Some(4), "other"), None);
    }
}
