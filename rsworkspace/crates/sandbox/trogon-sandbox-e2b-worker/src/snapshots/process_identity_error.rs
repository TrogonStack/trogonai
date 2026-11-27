use std::num::ParseIntError;
use std::str::Utf8Error;

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ProcessIdentityError {
    #[error("process identity exceeds the byte limit")]
    TooLong,
    #[error("process identity is not UTF-8")]
    NotUtf8(#[from] Utf8Error),
    #[error("process identity omitted pid")]
    MissingPid,
    #[error("process identity omitted start ticks")]
    MissingStart,
    #[error("process identity field is not a number")]
    NotNumeric(#[from] ParseIntError),
    #[error("invalid process identity")]
    Invalid,
}
