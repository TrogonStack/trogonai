use thiserror::Error;
use trogon_sts::StsTokenErrorResponse;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StsClientError {
    #[error("STS exchange timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("STS NATS request failed: {0}")]
    Transport(String),
    #[error("STS response decode failed: {0}")]
    Decode(String),
    #[error("STS rejected exchange: {error} ({error_description})")]
    ExchangeRejected {
        error: String,
        error_description: String,
    },
}

impl StsClientError {
    pub fn from_wire_error(resp: &StsTokenErrorResponse) -> Self {
        Self::ExchangeRejected {
            error: resp.error.clone(),
            error_description: resp.error_description.clone(),
        }
    }

    /// Map STS wire / transport failures to Trogon MCP gateway JSON-RPC codes.
    pub fn gateway_rpc_code(&self) -> i32 {
        match self {
            Self::Timeout(_) | Self::Transport(_) => -32_107,
            Self::Decode(_) => -32_110,
            Self::ExchangeRejected { error, .. } => match error.as_str() {
                "invalid_grant" => -32_106,
                "invalid_target" => -32_109,
                "rate_limited" => -32_105,
                "access_denied" => -32_100,
                "act_chain_depth_exceeded" => -32_113,
                "act_chain_loop_detected" => -32_114,
                "invalid_request" | "unsupported_grant_type" => -32_110,
                "unauthorized_client" => -32_117,
                "server_error" | "registry_unavailable" => -32_107,
                _ => -32_107,
            },
        }
    }

    pub fn gateway_rpc_message(&self) -> String {
        match self {
            Self::Timeout(_) => "sts_unavailable".into(),
            Self::Transport(_) => "sts_unavailable".into(),
            Self::Decode(_) => "invalid_token".into(),
            Self::ExchangeRejected { error, .. } => match error.as_str() {
                "invalid_target" => "audience_mismatch".into(),
                "invalid_grant" => "auth_expired".into(),
                "act_chain_depth_exceeded" => "act_chain_depth_exceeded".into(),
                "act_chain_loop_detected" => "act_chain_loop_detected".into(),
                "unauthorized_client" => "agent_identity_required".into(),
                "rate_limited" => "rate_limited".into(),
                "access_denied" => "policy_deny".into(),
                other => other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_target_maps_to_audience_mismatch() {
        let err = StsClientError::ExchangeRejected {
            error: "invalid_target".into(),
            error_description: "nope".into(),
        };
        assert_eq!(err.gateway_rpc_code(), -32_109);
        assert_eq!(err.gateway_rpc_message(), "audience_mismatch");
    }

    #[test]
    fn timeout_maps_to_authz_unreachable() {
        let err = StsClientError::Timeout(std::time::Duration::from_millis(100));
        assert_eq!(err.gateway_rpc_code(), -32_107);
    }
}
