/// JSON-RPC version constant re-injected on decode.
pub const JSONRPC_VERSION: &str = "2.0";

/// Header carrying the JSON-RPC `id` as a JSON literal.
pub const HEADER_ID: &str = "Jsonrpc-Id";

/// Header carrying the JSON-RPC `error.code`; presence is the success/error discriminator.
pub const HEADER_ERROR_CODE: &str = "Jsonrpc-Error-Code";
