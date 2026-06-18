//! Stream read and append contracts.
//!
//! These traits are storage-neutral ports. Concrete crates translate them to a
//! backend such as EventStoreDB, JetStream, Postgres, or an in-memory test
//! store while preserving the shared concurrency and position semantics.

mod append_stream;
mod read_stream;
mod stream_position;

pub use append_stream::{AppendStreamRequest, AppendStreamResponse, StreamAppend, StreamWritePrecondition};
pub use read_stream::{ReadAfterOverflow, ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamRead};
pub use stream_position::{InvalidStreamPosition, StreamPosition};

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    #[test]
    fn stream_position_conversions_preserve_non_zero_value() {
        let non_zero = NonZeroU64::new(42).unwrap();
        let position = StreamPosition::new(non_zero);

        assert_eq!(position.as_u64(), 42);
        assert_eq!(position.as_non_zero(), non_zero);
        assert_eq!(StreamPosition::try_new(42), Ok(position));
        assert_eq!(StreamPosition::try_from(42), Ok(position));
        assert_eq!(u64::from(position), 42);
        assert_eq!(position.to_string(), "42");

        let encoded = serde_json::to_string(&position).unwrap();
        assert_eq!(serde_json::from_str::<StreamPosition>(&encoded).unwrap(), position);
    }

    #[test]
    fn stream_position_rejects_zero_with_typed_error() {
        let error = StreamPosition::try_new(0).unwrap_err();

        assert_eq!(error.value(), 0);
        assert_eq!(error.to_string(), "stream position must be greater than zero, got 0");
        let _: &dyn std::error::Error = &error;
    }

    #[test]
    fn read_from_after_advances_position_and_reports_overflow() {
        let position = StreamPosition::try_new(7).unwrap();

        assert_eq!(
            ReadFrom::after(position),
            Ok(ReadFrom::Position(StreamPosition::try_new(8).unwrap()))
        );

        let max = StreamPosition::new(NonZeroU64::new(u64::MAX).unwrap());
        let error = ReadFrom::after(max).unwrap_err();
        assert_eq!(error.position, max);
        assert_eq!(
            error.to_string(),
            format!("cannot read after position {max}: u64 overflow")
        );
        let _: &dyn std::error::Error = &error;
    }

    #[test]
    fn write_precondition_from_observed_position_matches_stream_state() {
        let position = StreamPosition::try_new(3).unwrap();

        assert_eq!(
            StreamWritePrecondition::from(Some(position)),
            StreamWritePrecondition::At(position)
        );
        assert_eq!(StreamWritePrecondition::from(None), StreamWritePrecondition::NoStream);
    }
}
