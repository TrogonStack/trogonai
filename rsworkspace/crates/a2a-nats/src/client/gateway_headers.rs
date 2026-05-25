use async_nats::HeaderMap;

use a2a_auth_callout::{CallerJwtHeaderValue, MintedUserJwt, CALLER_JWT_HEADER_NAME};

use crate::constants::REQ_ID_HEADER;
use crate::req_id::ReqId;

use super::error::ClientError;

pub(crate) fn agent_rpc_headers(req_id: &ReqId) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id.as_str());
    headers
}

pub(crate) fn gateway_ingress_rpc_headers(req_id: &ReqId, caller_jwt: &MintedUserJwt) -> Result<HeaderMap, ClientError> {
    caller_jwt
        .ensure_fresh()
        .map_err(|e| ClientError::GatewayCallerJwtExpired(e.to_string()))?;
    let mut headers = HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id.as_str());
    let header_value = CallerJwtHeaderValue::from_minted(caller_jwt);
    headers.insert(CALLER_JWT_HEADER_NAME, header_value.as_str());
    Ok(headers)
}
