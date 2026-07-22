use async_nats::HeaderMap;

use a2a_identity_types::{CALLER_JWT_HEADER_NAME, CallerJwtHeaderValue, JwtError, MintedUserJwt};

use crate::constants::REQ_ID_HEADER;
use crate::req_id::ReqId;

use super::error::ClientError;

pub fn agent_rpc_headers(req_id: &ReqId) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id.as_str());
    headers
}

pub fn gateway_ingress_rpc_headers(req_id: &ReqId, caller_jwt: &MintedUserJwt) -> Result<HeaderMap, ClientError> {
    caller_jwt.ensure_fresh().map_err(jwt_freshness_to_client_error)?;
    let mut headers = HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id.as_str());
    let header_value = CallerJwtHeaderValue::from_minted(caller_jwt);
    headers.insert(CALLER_JWT_HEADER_NAME, header_value.as_str());
    Ok(headers)
}

/// Map a `JwtError` from `MintedUserJwt::ensure_fresh` onto the right
/// ClientError variant — only an exp-past-now failure is reported as
/// expired; missing `exp`, not-yet-valid `nbf`, decode issues, and clock
/// errors get the broader Invalid variant so refresh logic doesn't blindly
/// retry against a malformed JWT.
fn jwt_freshness_to_client_error(error: JwtError) -> ClientError {
    let msg = error.to_string();
    match &error {
        JwtError::Decode(detail) if detail == "user JWT expired" => ClientError::GatewayCallerJwtExpired(msg),
        _ => ClientError::GatewayCallerJwtInvalid(msg),
    }
}

#[cfg(test)]
mod tests;
