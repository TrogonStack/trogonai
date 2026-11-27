use axum::http::{HeaderMap, header};

use crate::constants::WS_TOKEN_PROTOCOL_PREFIX;

/// The session token a socket request presents: an `Authorization: Bearer` header, or a `bearer.<token>` entry in
/// the offered websocket subprotocols, which is what a browser can send.
pub(crate) fn extract_ws_token(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
    {
        return Some(token.to_string());
    }
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())?
        .split(',')
        .map(str::trim)
        .find_map(|protocol| {
            protocol
                .strip_prefix(WS_TOKEN_PROTOCOL_PREFIX)
                .filter(|token| !token.is_empty())
        })
        .map(str::to_string)
}
