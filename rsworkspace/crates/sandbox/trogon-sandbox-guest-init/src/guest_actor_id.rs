use std::fmt;

use crate::GuestActorIdError;

/// The id of the actor a bound guest serves, as named by the host on the kernel command line.
///
/// It is never empty and never the `unbound` marker, so writing it to the guest's actor file
/// cannot be confused with an unbound guest.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct GuestActorId(String);

impl GuestActorId {
    /// The marker an unbound guest records instead of an actor id.
    pub const UNBOUND_MARKER: &'static str = "unbound";

    pub fn new(value: impl Into<String>) -> Result<Self, GuestActorIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(GuestActorIdError::Empty);
        }
        if value == Self::UNBOUND_MARKER {
            return Err(GuestActorIdError::UnboundMarker);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GuestActorId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
