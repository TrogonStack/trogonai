use trogon_sandbox_contract::{LeaseError, ReadFileResponse};

use crate::ReadFileError;

/// Maps a read result onto the wire reply, turning [`ReadFileError::NotFound`] into `NotFound` and every other
/// failure into the worker's denial.
pub fn read_file_response<E>(
    result: Result<Vec<u8>, ReadFileError<E>>,
    denied: impl FnOnce(E) -> LeaseError,
) -> ReadFileResponse {
    match result {
        Ok(data) => ReadFileResponse::Content { data },
        Err(ReadFileError::NotFound(_)) => ReadFileResponse::NotFound,
        Err(ReadFileError::Other(error)) => ReadFileResponse::Denied(denied(error)),
    }
}
