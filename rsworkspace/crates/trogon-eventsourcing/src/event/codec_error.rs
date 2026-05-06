use std::fmt;

use crate::{EventCodec, EventType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError<DataError, MetadataError> {
    Data(DataError),
    Metadata(MetadataError),
}

impl<DataError, MetadataError> fmt::Display for CodecError<DataError, MetadataError>
where
    DataError: fmt::Display,
    MetadataError: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data(source) => write!(f, "{source}"),
            Self::Metadata(source) => write!(f, "{source}"),
        }
    }
}

impl<DataError, MetadataError> std::error::Error for CodecError<DataError, MetadataError>
where
    DataError: std::error::Error + 'static,
    MetadataError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Data(source) => Some(source),
            Self::Metadata(source) => Some(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeEventError<EventTypeError, EventCodecError> {
    EventType(EventTypeError),
    EventCodec(EventCodecError),
}

pub type EventDataEncodeError<E, C> = EncodeEventError<<E as EventType>::Error, <C as EventCodec<E>>::Error>;

pub type EventDataWithMetadataError<E, M, EC, MC> =
    CodecError<EventDataEncodeError<E, EC>, <MC as EventCodec<M>>::Error>;

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
