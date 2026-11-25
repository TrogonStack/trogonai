use async_nats::jetstream;

/// One invalidation notice as its consumer delivered it.
pub trait InvalidationNotice: Send + 'static {
    /// The subject it was published on.
    fn subject(&self) -> &str;

    /// Where it falls in what this consumer delivered, counting from one.
    fn consumer_sequence(&self) -> Result<u64, async_nats::Error>;
}

impl InvalidationNotice for jetstream::Message {
    fn subject(&self) -> &str {
        self.message.subject.as_str()
    }

    fn consumer_sequence(&self) -> Result<u64, async_nats::Error> {
        self.info().map(|info| info.consumer_sequence)
    }
}
