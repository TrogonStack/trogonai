use super::{ServiceErrorCode, ServiceErrorCodeError};
use crate::service_error_code_input::ServiceErrorCodeInput;
use buffa::Enumeration as _;
use trogonai_proto::google::rpc::Code;

#[test]
fn accepts_a_known_fault_code() {
    let code = ServiceErrorCode::new(Code::RESOURCE_EXHAUSTED.to_i32()).expect("a known non-OK code");
    assert_eq!(code.code(), Code::RESOURCE_EXHAUSTED);
}

#[test]
fn rejects_ok() {
    assert_eq!(
        ServiceErrorCode::new(Code::OK.to_i32()),
        Err(ServiceErrorCodeError::OkCode)
    );
}

#[test]
fn rejects_a_code_outside_the_enum() {
    assert_eq!(
        ServiceErrorCode::new(4242),
        Err(ServiceErrorCodeError::UnknownCode { value: 4242 })
    );
}

#[test]
fn rejects_a_header_that_is_not_an_integer() {
    let header = ServiceErrorCodeInput::new("RESOURCE_EXHAUSTED");
    assert_eq!(
        ServiceErrorCode::from_input(&header),
        Err(ServiceErrorCodeError::NotAnInteger { header })
    );
}

#[test]
fn reads_a_well_formed_header() {
    let header = ServiceErrorCodeInput::new(Code::NOT_FOUND.to_i32().to_string());
    let code = ServiceErrorCode::from_input(&header).expect("a well formed header");
    assert_eq!(code.code(), Code::NOT_FOUND);
}
