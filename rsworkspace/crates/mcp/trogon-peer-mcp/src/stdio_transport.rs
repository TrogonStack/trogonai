#[cfg(unix)]
use crate::native_pipe::NativePipe;

#[cfg(unix)]
pub(crate) fn native_stdio() -> std::io::Result<(NativePipe, NativePipe)> {
    Ok((NativePipe::new(std::io::stdin())?, NativePipe::new(std::io::stdout())?))
}

#[cfg(not(unix))]
pub(crate) fn native_stdio() -> std::io::Result<(tokio::io::Stdin, tokio::io::Stdout)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "cancellable native stdio is unavailable on this platform",
    ))
}
