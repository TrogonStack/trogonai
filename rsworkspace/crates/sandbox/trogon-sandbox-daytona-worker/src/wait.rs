use std::time::Duration;

/// How long, and how often, the worker looks at a sandbox that is changing state.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Wait {
    pub timeout: Duration,
    pub interval: Duration,
}
