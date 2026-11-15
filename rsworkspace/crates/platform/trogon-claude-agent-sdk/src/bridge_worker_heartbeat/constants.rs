use std::time::Duration;

/// The longest delay a timer can have; a longer one is run after the shortest (`2147483647`).
pub(super) const LARGEST_TIMER_MS: f64 = 2_147_483_647.0;

/// The least time between a beat and the one a probe asks for (`probeBeatSpacingMs`, `U2e`).
pub const PROBE_BEAT_SPACING: Duration = Duration::from_millis(5_000);
