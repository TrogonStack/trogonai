#[derive(serde::Deserialize)]
pub(crate) struct NativeStreamGetResponse {
    pub(crate) message: async_nats::jetstream::stream::RawMessage,
}
