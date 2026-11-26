use crate::DispatchAuthError;

/// Why a bounded NATS transfer failed.
#[derive(Debug, thiserror::Error)]
pub enum NatsTransferError {
    /// The payload exceeds the transfer cap.
    #[error("file transfer exceeds {max} encoded bytes")]
    TooLarge {
        /// The cap, in bytes.
        max: usize,
    },
    /// The server's maximum payload leaves no room for transfer metadata.
    #[error("NATS max_payload leaves no room for transfer metadata")]
    NoRoomForMetadata,
    /// The descriptor names a transfer version this peer does not speak.
    #[error("unknown file transfer version")]
    UnknownVersion,
    /// The descriptor's length is zero or above the transfer cap.
    #[error("invalid file transfer length")]
    InvalidLength,
    /// The descriptor's chunk size is zero or above the chunk cap.
    #[error("invalid file chunk size")]
    InvalidChunkSize,
    /// The descriptor's inbox is not a plain `_INBOX.` subject.
    #[error("invalid file transfer inbox")]
    InvalidInbox,
    /// A chunk reply did not have the expected length.
    #[error("invalid file chunk length")]
    InvalidChunkLength,
    /// The descriptor did not serialize or parse.
    #[error("invalid file transfer descriptor: {0}")]
    Descriptor(#[from] serde_json::Error),
    /// The chunk inbox subscription failed.
    #[error("failed to subscribe to the chunk inbox: {0}")]
    Subscribe(#[from] async_nats::client::SubscribeError),
    /// A request or chunk request failed.
    #[error("request failed: {0}")]
    Request(#[from] async_nats::client::RequestError),
    /// A publish failed.
    #[error("publish failed: {0}")]
    Publish(#[from] async_nats::client::PublishError),
    /// The reassembled payload failed dispatch authentication.
    #[error(transparent)]
    DispatchAuth(#[from] DispatchAuthError),
    /// The chunk sender did not finish in time.
    #[error("file chunk sender timed out")]
    SenderTimedOut,
    /// The chunk receiver did not finish in time.
    #[error("file chunk receiver timed out")]
    ReceiverTimedOut,
    /// The request did not complete in time.
    #[error("file request timed out")]
    RequestTimedOut,
    /// The chunk sender task did not run to completion.
    #[error("file chunk sender stopped: {0}")]
    SenderStopped(#[from] tokio::task::JoinError),
}
