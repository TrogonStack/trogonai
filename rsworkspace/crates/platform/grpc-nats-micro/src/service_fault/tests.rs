use super::ServiceFault;
use crate::service_error_code::{ServiceErrorCode, ServiceErrorCodeError};
use buffa::Enumeration as _;
use trogonai_proto::google::rpc::{Code, Status};

#[test]
fn rejects_an_ok_coded_status() {
    let status = Status {
        code: Code::OK.to_i32(),
        message: "not a fault".to_string(),
        details: Vec::new(),
    };
    assert_eq!(ServiceFault::new(status), Err(ServiceErrorCodeError::OkCode));
}

#[test]
fn the_authoritative_code_overrides_the_body() {
    let status = Status {
        code: Code::UNKNOWN.to_i32(),
        message: "out of quota".to_string(),
        details: Vec::new(),
    };
    let code = ServiceErrorCode::new(Code::RESOURCE_EXHAUSTED.to_i32()).expect("a known non-OK code");

    let fault = ServiceFault::with_code(code, status);

    assert_eq!(fault.code(), code);
    assert_eq!(fault.status().code, Code::RESOURCE_EXHAUSTED.to_i32());
}

#[test]
fn named_constructors_carry_their_code() {
    assert_eq!(
        ServiceFault::invalid_argument("bad").code().code(),
        Code::INVALID_ARGUMENT
    );
    assert_eq!(ServiceFault::internal("boom").code().code(), Code::INTERNAL);
}
