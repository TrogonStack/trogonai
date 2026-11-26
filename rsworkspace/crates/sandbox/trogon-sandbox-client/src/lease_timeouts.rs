use std::time::Duration;

/// The reply deadline for lease verbs that only update broker state.
pub(crate) const QUICK_VERB_TIMEOUT: Duration = Duration::from_secs(2);

/// The reply deadline for lease verbs that cover a worker freezing or thawing a sandbox.
pub(crate) const SLOW_VERB_TIMEOUT: Duration = Duration::from_secs(30);
