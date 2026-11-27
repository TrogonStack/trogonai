use tokio::io::{AsyncRead, AsyncWrite};

/// A client connection the explicit proxy can serve, whatever socket family
/// carried it.
pub trait ClientIo: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> ClientIo for T {}
