use std::time::Duration;

/// How long an observed binding stays live unless configured otherwise.
pub const DEFAULT_BINDING_TTL: Duration = Duration::from_secs(60);

/// The most bindings held at once; the oldest go first.
pub(super) const MAX_BINDINGS: usize = 16_384;
