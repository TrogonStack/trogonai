use crate::ProtocolError;
use crate::event_wire::{EventWire, validate_event};

/// A protocol event whose fields have passed validation.
///
/// The checked value cannot be overwritten through its wire view.
///
/// ```compile_fail
/// use sandbox_command_supervisor::protocol::{Event, EventWire};
/// fn overwrite(event: &mut Event, wire: EventWire) {
///     *event.as_wire() = wire;
/// }
/// ```
///
/// Deserialization must pass through the validating decoder.
///
/// ```compile_fail
/// use sandbox_command_supervisor::protocol::Event;
/// fn unchecked<T: serde::de::DeserializeOwned>() {}
/// unchecked::<Event>();
/// ```
#[derive(Clone)]
pub struct Event(EventWire);

impl Event {
    pub fn as_wire(&self) -> &EventWire {
        &self.0
    }

    pub fn into_wire(self) -> EventWire {
        self.0
    }
}

impl TryFrom<EventWire> for Event {
    type Error = ProtocolError;

    fn try_from(wire: EventWire) -> Result<Self, Self::Error> {
        validate_event(&wire)?;
        Ok(Self(wire))
    }
}

impl AsRef<EventWire> for Event {
    fn as_ref(&self) -> &EventWire {
        self.as_wire()
    }
}
