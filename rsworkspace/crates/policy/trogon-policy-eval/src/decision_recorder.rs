use trogon_decider_runtime::CommandPrincipal;

use crate::decision::Decision;
use crate::request::Request;

/// Receives every decision the authorizer makes, admitted or not.
///
/// A `DryRun` evaluator admits what it would have refused, and the only trace of
/// that is the decision's `would_deny`. Without a recorder it would be dropped.
/// The decision also lists every history consulted, which a caller records next
/// to the command (ADR#0071 Decision 7).
pub trait DecisionRecorder: Send + Sync {
    /// Called once per authorization, after the decision is made and before the
    /// authorizer answers.
    fn record(&self, principal: &CommandPrincipal, request: &Request, decision: &Decision);
}
