use std::fmt;

use crate::{EventEncode, EventType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeEventError<EventTypeError, EventEncodeError> {
    EventType(EventTypeError),
    EventEncode(EventEncodeError),
}

pub type EventEncodeError<E> = EncodeEventError<<E as EventType>::Error, <E as EventEncode>::Error>;

impl<EventTypeError, EventEncodeError> fmt::Display for EncodeEventError<EventTypeError, EventEncodeError>
where
    EventTypeError: fmt::Display,
    EventEncodeError: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventType(source) => write!(f, "{source}"),
            Self::EventEncode(source) => write!(f, "{source}"),
        }
    }
}

impl<EventTypeError, EventEncodeError> std::error::Error for EncodeEventError<EventTypeError, EventEncodeError>
where
    EventTypeError: std::error::Error + 'static,
    EventEncodeError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EventType(source) => Some(source),
            Self::EventEncode(source) => Some(source),
        }
    }
}
