#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]
//! Protocol Buffers request/reply over NATS micro (ADR 0016).
//!
//! This is not gRPC: there is no HTTP/2, no gRPC wire framing, and no gRPC
//! library on the request/reply path. "gRPC" in the crate name is a naming
//! idiom only; transport is a NATS micro service (NATS Services / ADR-32),
//! and the wire payload is either protobuf binary or canonical proto3 JSON,
//! negotiated per `Content-Type` (see [`content_type`]).
//!
//! See `docs/adr/0016-protobuf-rpc-over-nats-micro-binding.md` for the full
//! binding specification this crate implements.

pub mod binding;
pub mod client;
pub mod constants;
pub mod content_type;
pub mod content_type_input;
pub mod endpoint_subject;
pub mod method_name;
pub mod server;
pub mod service_error_code;
pub mod service_error_code_input;
pub mod service_fault;
pub mod service_name;
pub mod service_version;
pub mod status_codec;
pub mod subject_prefix;

pub use binding::{EndpointBinding, ServiceBinding};
pub use content_type::ContentType;
pub use content_type_input::ContentTypeInput;
pub use endpoint_subject::{EndpointSubject, EndpointSubjectError};
pub use method_name::{MethodName, MethodNameError};
pub use server::{EndpointHandler, ServeError, serve};
pub use service_error_code::{ServiceErrorCode, ServiceErrorCodeError};
pub use service_error_code_input::ServiceErrorCodeInput;
pub use service_fault::ServiceFault;
pub use service_name::{ServiceName, ServiceNameError};
pub use service_version::{ServiceVersion, ServiceVersionError};
pub use status_codec::{EncodedReply, Outcome, ReplyError, ServiceError};
pub use subject_prefix::{SubjectPrefix, SubjectPrefixError};
