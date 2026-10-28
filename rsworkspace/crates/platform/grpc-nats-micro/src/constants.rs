/// Header present iff a reply is a micro service error (ADR 0016 §3).
pub const HEADER_ERROR_CODE: &str = "Nats-Service-Error-Code";

/// Header carrying the developer-facing error message; mirrors `Status.message`.
pub const HEADER_ERROR: &str = "Nats-Service-Error";

/// Header negotiating the request/reply payload encoding (ADR 0016 §4).
pub const HEADER_CONTENT_TYPE: &str = "Content-Type";

/// `Content-Type` value for the protobuf binary wire encoding.
pub const CONTENT_TYPE_PROTOBUF: &str = "application/protobuf";

/// `Content-Type` value for canonical proto3 JSON.
pub const CONTENT_TYPE_JSON: &str = "application/json";

/// Default NATS micro queue group (ADR 0016 §5).
pub const DEFAULT_QUEUE_GROUP: &str = "q";
