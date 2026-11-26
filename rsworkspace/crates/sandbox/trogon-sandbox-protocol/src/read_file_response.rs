use trogon_sandbox_contract::{LeaseError, ReadFileResponse};

use crate::ReadFileFailure;

/// Maps a read result onto the wire reply, turning [`ReadFileFailure::NotFound`] into `NotFound` and every other
/// failure into the worker's denial.
pub fn read_file_response<E>(
    result: Result<Vec<u8>, ReadFileFailure<E>>,
    denied: impl FnOnce(E) -> LeaseError,
) -> ReadFileResponse {
    match result {
        Ok(data) => ReadFileResponse::Content { data },
        Err(ReadFileFailure::NotFound(_)) => ReadFileResponse::NotFound,
        Err(ReadFileFailure::Other(error)) => ReadFileResponse::Denied(denied(error)),
    }
}
