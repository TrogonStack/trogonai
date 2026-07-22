use std::panic::{AssertUnwindSafe, catch_unwind};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("widget event codec failure")]
struct WidgetEventCodecError;

#[derive(Debug, Clone, PartialEq, Eq)]
enum WidgetEvent {
    Created { name: String },
    Renamed { name: String },
}

impl EventType for WidgetEvent {
    type Error = WidgetEventCodecError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        match self {
            Self::Created { .. } => Ok("widget.created.v2"),
            Self::Renamed { .. } => Ok("widget.renamed"),
        }
    }
}

impl EventEncode for WidgetEvent {
    type Error = WidgetEventCodecError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        match self {
            Self::Created { name } | Self::Renamed { name } => Ok(name.clone().into_bytes()),
        }
    }
}

impl EventDecode for WidgetEvent {
    type Error = WidgetEventCodecError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        let name = String::from_utf8(event.payload.to_vec()).map_err(|_| WidgetEventCodecError)?;
        Ok(match event.event_type {
            "widget.created.v2" => EventDecodeOutcome::Decoded(Self::Created { name }),
            // Historical alias kept so envelopes written before the v2 rename still replay.
            "widget.created" => EventDecodeOutcome::Decoded(Self::Created { name }),
            "widget.renamed" => EventDecodeOutcome::Decoded(Self::Renamed { name }),
            _ => EventDecodeOutcome::Skipped,
        })
    }
}

#[test]
fn assert_round_trips_passes_for_current_event_type() {
    assert_round_trips(WidgetEvent::Created {
        name: "widget-1".to_string(),
    });
    assert_round_trips(WidgetEvent::Renamed {
        name: "widget-2".to_string(),
    });
}

#[test]
fn aliased_event_type_decodes_to_the_current_variant() {
    let decoded = WidgetEvent::decode(EventData::new("widget.created", b"widget-1")).unwrap();

    assert_eq!(
        decoded,
        EventDecodeOutcome::Decoded(WidgetEvent::Created {
            name: "widget-1".to_string(),
        })
    );
}

#[test]
fn unrecognized_event_type_is_skipped_not_errored() {
    let decoded = WidgetEvent::decode(EventData::new("other.event", b"payload")).unwrap();

    assert_eq!(decoded, EventDecodeOutcome::Skipped);
}

#[test]
fn assert_round_trips_panics_when_decoder_skips_the_event_type() {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct AlwaysSkips;

    impl EventType for AlwaysSkips {
        type Error = WidgetEventCodecError;

        fn event_type(&self) -> Result<&'static str, Self::Error> {
            Ok("always.skips")
        }
    }

    impl EventEncode for AlwaysSkips {
        type Error = WidgetEventCodecError;

        fn encode(&self) -> Result<Vec<u8>, Self::Error> {
            Ok(Vec::new())
        }
    }

    impl EventDecode for AlwaysSkips {
        type Error = WidgetEventCodecError;

        fn decode(_event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
            Ok(EventDecodeOutcome::Skipped)
        }
    }

    let panic = catch_unwind(AssertUnwindSafe(|| assert_round_trips(AlwaysSkips))).unwrap_err();
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();

    assert!(message.contains("did not match the original after a codec round trip"));
}
