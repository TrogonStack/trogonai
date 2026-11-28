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

/// Encodes a file read, refusing content that cannot travel in one transfer of at most `max_payload` bytes.
pub fn encode_read_file_response_within(
    response: &ReadFileResponse,
    path: &str,
    max_payload: usize,
) -> serde_json::Result<Vec<u8>> {
    let too_large = |data: &[u8]| {
        serde_json::to_vec(&ReadFileResponse::Denied(LeaseError::FileTooLargeForTransport {
            path: path.to_owned(),
            bytes: data.len() as u64,
            max_payload_bytes: max_payload as u64,
        }))
    };
    // The JSON encoding of the bytes is never shorter than the bytes themselves, so content already past the limit
    // is refused without encoding it.
    if let ReadFileResponse::Content { data } = response
        && data.len() > max_payload
    {
        return too_large(data);
    }
    let body = serde_json::to_vec(response)?;
    if let ReadFileResponse::Content { data } = response
        && body.len() > max_payload
    {
        return too_large(data);
    }
    Ok(body)
}
