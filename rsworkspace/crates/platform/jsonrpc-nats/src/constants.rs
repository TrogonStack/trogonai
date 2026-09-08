/// JSON-RPC version constant re-injected on decode.
pub const JSONRPC_VERSION: &str = "2.0";

/// Header carrying the JSON-RPC `id` as a JSON literal.
pub const HEADER_ID: &str = "Jsonrpc-Id";

/// Header carrying the JSON-RPC `error.code`; presence is the success/error discriminator.
pub const HEADER_ERROR_CODE: &str = "Jsonrpc-Error-Code";

/// Invalid JSON was received (JSON-RPC 2.0 section 5.1).
pub const PARSE_ERROR: i32 = -32700;

/// The JSON sent is not a valid Request object (JSON-RPC 2.0 section 5.1).
pub const INVALID_REQUEST: i32 = -32600;

/// The method does not exist or is not available (JSON-RPC 2.0 section 5.1).
pub const METHOD_NOT_FOUND: i32 = -32601;

/// Invalid method parameters (JSON-RPC 2.0 section 5.1).
pub const INVALID_PARAMS: i32 = -32602;

/// Internal JSON-RPC error (JSON-RPC 2.0 section 5.1).
pub const INTERNAL_ERROR: i32 = -32603;
