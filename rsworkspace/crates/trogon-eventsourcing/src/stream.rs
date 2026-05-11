mod append;
mod position;
mod read;
mod state;

pub use append::{AppendStreamRequest, AppendStreamResponse};
pub use position::{InvalidStreamPosition, StreamPosition};
pub use read::{ReadStreamRequest, ReadStreamResponse};
pub use state::StreamState;

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
