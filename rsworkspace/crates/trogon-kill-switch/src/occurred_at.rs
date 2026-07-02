use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Wall-clock instant an aggregate transition took effect.
///
/// Wraps [`OffsetDateTime`] so events carry a validated, RFC 3339-roundtrippable
/// timestamp rather than a bare `String` or unchecked `OffsetDateTime` that could
/// hold sub/nanosecond precision the wire format silently drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OccurredAt(OffsetDateTime);

#[derive(Debug, thiserror::Error)]
pub enum OccurredAtError {
    #[error("failed to format timestamp as RFC 3339: {0}")]
    Format(#[source] time::error::Format),
    #[error("failed to parse timestamp as RFC 3339: {0}")]
    Parse(#[source] time::error::Parse),
}

impl OccurredAt {
    pub fn new(instant: OffsetDateTime) -> Self {
        Self(instant)
    }

    pub fn now() -> Self {
        Self(OffsetDateTime::now_utc())
    }

    pub fn to_rfc3339(self) -> Result<String, OccurredAtError> {
        self.0.format(&Rfc3339).map_err(OccurredAtError::Format)
    }

    pub fn parse_rfc3339(raw: &str) -> Result<Self, OccurredAtError> {
        OffsetDateTime::parse(raw, &Rfc3339)
            .map(Self)
            .map_err(OccurredAtError::Parse)
    }

    pub const fn as_offset_date_time(self) -> OffsetDateTime {
        self.0
    }
}

#[cfg(test)]
mod tests;
