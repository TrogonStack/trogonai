use std::time::Duration;

/// How far ahead a no-TTL lease's retained-snapshot reservation expires. It
/// matches the broker's far deadline for such leases, so only a release or
/// the broker's idle reaper ends the reservation.
pub const NO_TTL_RESERVATION_SPAN: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);
