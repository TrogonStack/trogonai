/// Header name carrying a serialized [`crate::jwt::CallerJwtHeaderValue`] on
/// every A2A request, including gateway-mediated traffic.
pub const CALLER_JWT_HEADER_NAME: &str = "A2a-Caller-Jwt";
