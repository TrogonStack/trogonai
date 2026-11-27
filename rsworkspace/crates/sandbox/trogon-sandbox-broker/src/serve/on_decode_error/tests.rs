use std::error::Error;

use trogon_sandbox_contract::{LeaseRequest, LeaseResponse};

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn decode_error() -> Result<serde_json::Error, Box<dyn Error>> {
    match serde_json::from_slice::<LeaseRequest>(b"not json") {
        Ok(_) => Err("garbage decoded as a lease request".into()),
        Err(error) => Ok(error),
    }
}

#[test]
fn reply_carries_the_decode_error_as_a_backend_denial() -> TestResult {
    let error = decode_error()?;
    let policy: OnDecodeError<LeaseResponse> = OnDecodeError::Reply(LeaseResponse::Denied);
    let response = policy.response::<LeaseRequest>(&error);
    let expected = LeaseResponse::Denied(LeaseError::Backend {
        message: format!("decode error: {error}"),
    });
    assert_eq!(response, Some(expected));
    Ok(())
}

#[test]
fn warn_and_ignore_send_nothing_back() -> TestResult {
    let error = decode_error()?;
    let warn: OnDecodeError<LeaseResponse> = OnDecodeError::Warn;
    let ignore: OnDecodeError<LeaseResponse> = OnDecodeError::Ignore;
    assert_eq!(warn.response::<LeaseRequest>(&error), None);
    assert_eq!(ignore.response::<LeaseRequest>(&error), None);
    Ok(())
}
