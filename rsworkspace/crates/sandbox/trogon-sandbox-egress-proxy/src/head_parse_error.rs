/// What a strict HTTP parser says of a head it cannot read, as it is quoted
/// back to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HeadParseError {
    #[error("Invalid method encountered")]
    InvalidMethod,
    #[error("Invalid header token")]
    InvalidHeaderToken,
    #[error("Invalid character in Content-Length")]
    InvalidContentLength,
    #[error("Duplicate Content-Length")]
    DuplicateContentLength,
    #[error("Unexpected whitespace after header value")]
    WhitespaceAfterHeaderValue,
    #[error("Expected CRLF after version")]
    ExpectedCrlf,
    #[error("Header overflow")]
    HeaderOverflow,
    #[error("Expected space after method")]
    ExpectedSpaceAfterMethod,
    #[error("Invalid method for HTTP/x.x request")]
    InvalidMethodForHttp,
    #[error("Invalid method for RTSP/x.x request")]
    InvalidMethodForRtsp,
    #[error("Expected SOURCE method for ICE/x.x request")]
    ExpectedSourceForIce,
    #[error("Expected HTTP/, RTSP/ or ICE/")]
    ExpectedProtocol,
    #[error("Invalid major version")]
    InvalidMajorVersion,
    #[error("Expected dot")]
    ExpectedDot,
    #[error("Invalid minor version")]
    InvalidMinorVersion,
    #[error("Invalid HTTP version")]
    InvalidVersion,
    #[error("Invalid header field char")]
    InvalidHeaderFieldChar,
    #[error("Missing expected CR after header value")]
    MissingCrAfterHeaderValue,
    #[error("Empty Transfer-Encoding")]
    EmptyTransferEncoding,
    #[error("Invalid `Transfer-Encoding` header value")]
    InvalidTransferEncodingValue,
    #[error("Content-Length can't be present with Transfer-Encoding")]
    ContentLengthAfterTransferEncoding,
    #[error("Transfer-Encoding can't be present with Content-Length")]
    TransferEncodingAfterContentLength,
    #[error("Request has invalid `Transfer-Encoding`")]
    UnchunkedTransferEncoding,
    #[error("Invalid character in chunk size")]
    InvalidChunkSize,
    #[error("Expected LF after chunk data")]
    ExpectedLfAfterChunkData,
    #[error("Invalid URL")]
    InvalidUrl,
    #[error("Expected HTTP/2 Connection Preface")]
    ExpectedPreface,
    #[error("Pause on PRI/Upgrade")]
    PausedUpgrade,
    #[error("Chunk extensions overflow")]
    ChunkExtensionsOverflow,
    #[error("Invalid character in chunk extensions")]
    InvalidChunkExtensions,
    #[error("Data after `Connection: close`")]
    DataAfterClose,
}
