//! The `google.rpc.Code` of an error reply: known, and never `OK`.

use buffa::Enumeration as _;
use thiserror::Error;
use trogonai_proto::google::rpc::Code;

use crate::service_error_code_input::ServiceErrorCodeInput;

/// Why a wire value cannot describe a service error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServiceErrorCodeError {
    #[error("service error code {header} is not an integer")]
    NotAnInteger { header: ServiceErrorCodeInput },
    #[error("service error code {value} is not a google.rpc.Code")]
    UnknownCode { value: i32 },
    #[error("google.rpc.Code OK cannot describe a service error")]
    OkCode,
}

/// A `google.rpc.Code` an error reply may carry. ADR 0016 §3 makes the error
/// channel's code space exclude `OK`, so an `OK`-coded fault is not
/// representable rather than repaired downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceErrorCode(Code);

impl ServiceErrorCode {
    /// The codes this binding raises itself, which the transport needs
    /// without a fallible construction step.
    pub const INTERNAL: Self = Self(Code::INTERNAL);
    pub const INVALID_ARGUMENT: Self = Self(Code::INVALID_ARGUMENT);

    pub fn new(value: i32) -> Result<Self, ServiceErrorCodeError> {
        let code = Code::from_i32(value).ok_or(ServiceErrorCodeError::UnknownCode { value })?;
        if code == Code::OK {
            return Err(ServiceErrorCodeError::OkCode);
        }
        Ok(Self(code))
    }

    pub fn from_input(input: &ServiceErrorCodeInput) -> Result<Self, ServiceErrorCodeError> {
        let value: i32 = input
            .as_str()
            .parse()
            .map_err(|_| ServiceErrorCodeError::NotAnInteger { header: input.clone() })?;
        Self::new(value)
    }

    pub const fn code(self) -> Code {
        self.0
    }

    pub fn to_i32(self) -> i32 {
        self.0.to_i32()
    }
}

impl std::fmt::Display for ServiceErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

#[cfg(test)]
mod tests;
