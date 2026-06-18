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
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    use super::*;

    fn build_token(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"alg":"HS256","typ":"JWT"})).unwrap());
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("{header}.{body}.signature")
    }

    #[test]
    fn gateway_ingress_rpc_headers_sets_req_id_and_caller_jwt_on_fresh_jwt() {
        let token = build_token(json!({ "exp": 9_999_999_999i64 }));
        let jwt = MintedUserJwt::new(token.clone()).unwrap();
        let req_id = ReqId::from_test("r-2");
        let headers = gateway_ingress_rpc_headers(&req_id, &jwt).unwrap();
        assert_eq!(headers.get(REQ_ID_HEADER).unwrap().as_str(), "r-2");
        assert_eq!(headers.get(CALLER_JWT_HEADER_NAME).unwrap().as_str(), token);
    }

    #[test]
    fn gateway_ingress_rpc_headers_returns_expired_on_past_exp() {
        let token = build_token(json!({ "exp": 1i64 }));
        let jwt = MintedUserJwt::new(token).unwrap();
        let req_id = ReqId::from_test("r-3");
        let err = gateway_ingress_rpc_headers(&req_id, &jwt).unwrap_err();
        assert!(matches!(err, ClientError::GatewayCallerJwtExpired(_)));
    }

    #[test]
    fn gateway_ingress_rpc_headers_returns_invalid_on_missing_exp() {
        let token = build_token(json!({}));
        let jwt = MintedUserJwt::new(token).unwrap();
        let req_id = ReqId::from_test("r-4");
        let err = gateway_ingress_rpc_headers(&req_id, &jwt).unwrap_err();
        assert!(matches!(err, ClientError::GatewayCallerJwtInvalid(_)));
    }

    #[test]
    fn agent_rpc_headers_set_req_id_only() {
        let req_id = ReqId::from_test("r-1");
        let headers = agent_rpc_headers(&req_id);
        assert_eq!(headers.get(REQ_ID_HEADER).unwrap().as_str(), "r-1");
        assert!(headers.get(CALLER_JWT_HEADER_NAME).is_none());
    }

    #[test]
    fn jwt_freshness_routes_expired_to_expired_variant() {
        let err = jwt_freshness_to_client_error(JwtError::Decode("user JWT expired".into()));
        assert!(matches!(err, ClientError::GatewayCallerJwtExpired(_)));
    }

    #[test]
    fn jwt_freshness_routes_missing_exp_to_invalid_variant() {
        let err = jwt_freshness_to_client_error(JwtError::Decode("user JWT missing exp".into()));
        assert!(matches!(err, ClientError::GatewayCallerJwtInvalid(_)));
    }

    #[test]
    fn jwt_freshness_routes_not_yet_valid_to_invalid_variant() {
        let err = jwt_freshness_to_client_error(JwtError::Decode("user JWT not yet valid".into()));
        assert!(matches!(err, ClientError::GatewayCallerJwtInvalid(_)));
    }

    #[test]
    fn jwt_freshness_routes_issued_at_out_of_range_to_invalid_variant() {
        let err = jwt_freshness_to_client_error(JwtError::IssuedAtOutOfRange);
        assert!(matches!(err, ClientError::GatewayCallerJwtInvalid(_)));
    }
}
