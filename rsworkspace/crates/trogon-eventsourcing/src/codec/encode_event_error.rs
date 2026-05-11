use std::fmt;

use crate::{EventCodec, EventType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeEventError<EventTypeError, EventCodecError> {
    EventType(EventTypeError),
    EventCodec(EventCodecError),
}

pub type EventDataEncodeError<E, C> = EncodeEventError<<E as EventType>::Error, <C as EventCodec<E>>::Error>;

impl<EventTypeError, EventCodecError> fmt::Display for EncodeEventError<EventTypeError, EventCodecError>
where
    EventTypeError: fmt::Display,
    EventCodecError: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventType(source) => write!(f, "{source}"),
            Self::EventCodec(source) => write!(f, "{source}"),
        }
    }
}

impl<EventTypeError, EventCodecError> std::error::Error for EncodeEventError<EventTypeError, EventCodecError>
where
    EventTypeError: std::error::Error + 'static,
    EventCodecError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EventType(source) => Some(source),
            Self::EventCodec(source) => Some(source),
        }
    }
}
