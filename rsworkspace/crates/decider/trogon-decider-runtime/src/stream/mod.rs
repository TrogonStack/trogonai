//! Stream read and append contracts.
//!
//! These traits are storage-neutral ports. Concrete crates translate them to a
//! backend such as EventStoreDB, JetStream, Postgres, or an in-memory test
//! store while preserving the shared concurrency and position semantics.

mod append_stream;
mod read_stream;
mod stream_position;

pub use append_stream::{AppendStreamRequest, AppendStreamResponse, StreamAppend, StreamWritePrecondition};
pub use read_stream::{ReadAfterOverflowError, ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamRead};
pub use stream_position::{InvalidStreamPositionError, StreamPosition};

#[cfg(test)]
mod tests;
