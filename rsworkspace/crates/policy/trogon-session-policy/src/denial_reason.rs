#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenialReason {
    SessionClosed,
    NewSessionRequired,
    RouteUndeclared,
    ToolUndeclared,
    ShellDisabled,
    PayloadLimit,
    BudgetExhausted,
    ReviewVeto,
    ReviewUnavailable,
}
