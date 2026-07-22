use std::fmt::Debug;

use crate::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

/// Asserts that an event survives a codec round trip: [`EventEncode::encode`] followed
/// by [`EventDecode::decode`] against the event's own [`EventType::event_type`].
///
/// This exercises the same boundary the storage adapter crosses at runtime, so it
/// catches codec bugs a pure `evolve`/`decide` test cannot: a decoder that does not
/// recognize its own encoder's event type, a payload that does not survive
/// serialization, or a decoder that resolves to [`EventDecodeOutcome::Skipped`]
/// instead of [`EventDecodeOutcome::Decoded`].
///
/// It does not exercise aliases, migrations, or upcasters on their own: those map a
/// *historical* event type string to the current event, so assert them directly by
/// building [`EventData`] with the historical type and calling
/// [`EventDecode::decode`].
///
/// # Example
///
/// ```
/// use trogon_decider::testing::assert_round_trips;
/// use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};
///
/// #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
/// #[error("widget event codec failure")]
/// struct WidgetEventCodecError;
///
/// #[derive(Debug, Clone, PartialEq, Eq)]
/// enum WidgetEvent {
///     Created { name: String },
/// }
///
/// impl EventType for WidgetEvent {
///     type Error = WidgetEventCodecError;
///
///     fn event_type(&self) -> Result<&'static str, Self::Error> {
///         match self {
///             Self::Created { .. } => Ok("widget.created.v2"),
///         }
///     }
/// }
///
/// impl EventEncode for WidgetEvent {
///     type Error = WidgetEventCodecError;
///
///     fn encode(&self) -> Result<Vec<u8>, Self::Error> {
///         match self {
///             Self::Created { name } => Ok(name.clone().into_bytes()),
///         }
///     }
/// }
///
/// impl EventDecode for WidgetEvent {
///     type Error = WidgetEventCodecError;
///
///     fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
///         let name = String::from_utf8(event.payload.to_vec()).map_err(|_| WidgetEventCodecError)?;
///         Ok(match event.event_type {
///             // Current wire name.
///             "widget.created.v2" => EventDecodeOutcome::Decoded(Self::Created { name }),
///             // Historical alias kept so old envelopes still replay.
///             "widget.created" => EventDecodeOutcome::Decoded(Self::Created { name }),
///             _ => EventDecodeOutcome::Skipped,
///         })
///     }
/// }
///
/// assert_round_trips(WidgetEvent::Created { name: "widget-1".to_string() });
///
/// // Aliases are asserted separately, against the historical event type string.
/// let aliased = WidgetEvent::decode(EventData::new("widget.created", b"widget-1")).unwrap();
/// assert_eq!(aliased, EventDecodeOutcome::Decoded(WidgetEvent::Created { name: "widget-1".to_string() }));
/// ```
pub fn assert_round_trips<E>(event: E)
where
    E: EventType + EventEncode + EventDecode + PartialEq + Debug,
{
    let event_type = event
        .event_type()
        .unwrap_or_else(|error| panic!("assert_round_trips(...) failed to resolve event type: {error}"));
    let payload = event
        .encode()
        .unwrap_or_else(|error| panic!("assert_round_trips(...) failed to encode event: {error}"));
    let decoded = E::decode(EventData::new(event_type, &payload))
        .unwrap_or_else(|error| panic!("assert_round_trips(...) failed to decode event: {error}"));

    assert_eq!(
        decoded,
        EventDecodeOutcome::Decoded(event),
        "assert_round_trips(...) decoded event did not match the original after a codec round trip"
    );
}

#[cfg(test)]
mod tests;
