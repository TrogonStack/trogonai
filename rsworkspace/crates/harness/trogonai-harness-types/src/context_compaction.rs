//! Compaction transaction shapes shared outside the agent runtime.

use crate::history::RetainedCompactionWindow;

/// A compaction transaction the caller owns after a successful run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCompactionTransactionResult {
    /// The bounded handoff block compaction produced.
    pub handoff: String,
    /// Token budget the plan accepted for that handoff.
    pub accepted_tokens: usize,
}

impl RetainedCompactionWindow {
    /// Halves a retention target when manual compaction still cannot fit.
    ///
    /// Measuring the fixed continuation cost and building the manual compaction plan stay in
    /// `trogonai-harness-agent`; this is the pure tail that decides whether to refine the
    /// target and by how much once the caller knows the plan outcome.
    #[must_use]
    pub fn refine_budget(&self, target: &mut usize, plan_accepts_handoff: bool) -> bool {
        if *target == 0 {
            return false;
        }
        if plan_accepts_handoff {
            return false;
        }
        *target = if self.retained_tokens <= self.newest_exchange_tokens {
            0
        } else {
            *target / 2
        };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Message;
    use crate::history::{ContextHistoryCut, RetainedCompactionWindow};

    #[test]
    fn refine_budget_halves_the_target_when_the_plan_still_rejects_the_window() {
        let window = RetainedCompactionWindow {
            source: vec![Message::user("source")],
            retained_history: Vec::new(),
            retained_messages: vec![Message::user("kept")],
            cut: ContextHistoryCut {
                turns: 1,
                ..ContextHistoryCut::default()
            },
            newest_exchange_tokens: 100,
            retained_tokens: 500,
        };
        let mut target = 800;

        assert!(window.refine_budget(&mut target, false));
        assert_eq!(target, 400);
    }

    #[test]
    fn refine_budget_zeros_the_target_when_only_the_newest_exchange_remains() {
        let window = RetainedCompactionWindow {
            source: Vec::new(),
            retained_history: Vec::new(),
            retained_messages: Vec::new(),
            cut: ContextHistoryCut::default(),
            newest_exchange_tokens: 500,
            retained_tokens: 500,
        };
        let mut target = 800;

        assert!(window.refine_budget(&mut target, false));
        assert_eq!(target, 0);
    }

    #[test]
    fn refine_budget_leaves_a_zero_target_alone() {
        let window = RetainedCompactionWindow {
            source: Vec::new(),
            retained_history: Vec::new(),
            retained_messages: Vec::new(),
            cut: ContextHistoryCut::default(),
            newest_exchange_tokens: 0,
            retained_tokens: 0,
        };
        let mut target = 0;

        assert!(!window.refine_budget(&mut target, false));
        assert_eq!(target, 0);
    }

    #[test]
    fn refine_budget_stops_when_the_plan_already_accepts_a_handoff() {
        let window = RetainedCompactionWindow {
            source: Vec::new(),
            retained_history: Vec::new(),
            retained_messages: Vec::new(),
            cut: ContextHistoryCut::default(),
            newest_exchange_tokens: 100,
            retained_tokens: 500,
        };
        let mut target = 800;

        assert!(!window.refine_budget(&mut target, true));
        assert_eq!(target, 800);
    }

    #[test]
    fn context_compaction_transaction_result_carries_the_handoff_and_budget() {
        let result = ContextCompactionTransactionResult {
            handoff: "<context_handoff>summary</context_handoff>".to_owned(),
            accepted_tokens: 512,
        };

        assert_eq!(result.accepted_tokens, 512);
        assert!(result.handoff.contains("summary"));
    }
}
