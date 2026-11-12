use crate::{HistoryError, constants::MAX_HISTORY_EVENTS};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, StreamAppend, StreamRead,
};
use trogon_peer::PeerId;

pub(crate) struct ResumeStore<'a, Store>(pub(crate) &'a Store);

impl<Store: StreamRead<PeerId>> StreamRead<PeerId> for ResumeStore<'_, Store> {
    type Error = HistoryError<Store::Error>;

    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        let read = self
            .0
            .read_stream_bounded(request, MAX_HISTORY_EVENTS + 1)
            .await
            .map_err(HistoryError::Read)?;
        crate::history::validate(&read)?;
        Ok(read)
    }
}

impl<Store: StreamAppend<PeerId>> StreamAppend<PeerId> for ResumeStore<'_, Store> {
    type Error = Store::Error;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        self.0.append_stream(request).await
    }
}
