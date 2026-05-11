mod append_stream_request;
mod append_stream_response;
mod read_stream_request;
mod read_stream_response;
mod stream_position;
mod stream_state;

pub use append_stream_request::AppendStreamRequest;
pub use append_stream_response::AppendStreamResponse;
pub use read_stream_request::ReadStreamRequest;
pub use read_stream_response::ReadStreamResponse;
pub use stream_position::{InvalidStreamPosition, StreamPosition};
pub use stream_state::StreamState;

pub trait StreamRead<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn read_stream(
        &self,
        request: ReadStreamRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<ReadStreamResponse, Self::Error>> + Send;
}

pub trait StreamAppend<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn append_stream(
        &self,
        request: AppendStreamRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<AppendStreamResponse, Self::Error>> + Send;
}
