//! A fault an endpoint reports on the micro error channel (ADR 0016 §3).

use trogonai_proto::google::rpc::Status;

use crate::service_error_code::{ServiceErrorCode, ServiceErrorCodeError};

/// A `google.rpc.Status` whose code is a valid service error code, so an
/// error reply cannot be emitted without one. `details` travels with it
/// because ADR 0016 §3 makes the body the only place `details` is readable.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceFault {
    code: ServiceErrorCode,
    status: Status,
}

impl ServiceFault {
    pub fn new(status: Status) -> Result<Self, ServiceErrorCodeError> {
        let code = ServiceErrorCode::new(status.code)?;
        Ok(Self { code, status })
    }

    /// Build a fault from a body and the code the transport says is
    /// authoritative, which ADR 0016 §3 makes the `Nats-Service-Error-Code`
    /// header on disagreement with the body.
    pub fn with_code(code: ServiceErrorCode, mut status: Status) -> Self {
        status.code = code.to_i32();
        Self { code, status }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::of(ServiceErrorCode::INVALID_ARGUMENT, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::of(ServiceErrorCode::INTERNAL, message)
    }

    fn of(code: ServiceErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            status: Status {
                code: code.to_i32(),
                message: message.into(),
                details: Vec::new(),
            },
        }
    }

    pub const fn code(&self) -> ServiceErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.status.message
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn into_status(self) -> Status {
        self.status
    }
}

#[cfg(test)]
mod tests;
