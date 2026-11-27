use std::time::Duration;

use async_nats::client::RequestErrorKind;
use bytes::Bytes;
use trogon_sandbox_contract::{GetAccountLimitsRequest, GetAccountLimitsResponse, TenantId};
use trogon_sandbox_protocol::subjects::TENANT_LIMITS_REQUEST;

use crate::DevError;

/// Waits until a TCP connection to `addr` succeeds. `hint` is appended to the error.
pub async fn wait_for_tcp(
    service: &'static str,
    addr: &str,
    timeout: Duration,
    hint: &'static str,
) -> Result<(), DevError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(DevError::NotListening {
                service,
                addr: addr.to_owned(),
                timeout,
                hint,
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Waits until the broker answers a tenant limits request for `tenant`.
pub async fn wait_for_broker(nats_url: &str, tenant: &TenantId, timeout: Duration) -> Result<(), DevError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let client = tokio::time::timeout_at(deadline, async_nats::connect(nats_url))
        .await
        .map_err(|_| DevError::BrokerDeadline)?
        .map_err(DevError::NatsConnect)?;
    let payload = Bytes::from(
        serde_json::to_vec(&GetAccountLimitsRequest { tenant: tenant.clone() })
            .map_err(DevError::EncodeBrokerRequest)?,
    );
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(DevError::BrokerDeadline);
        }
        let reply = tokio::time::timeout_at(deadline, client.request(TENANT_LIMITS_REQUEST, payload.clone()))
            .await
            .map_err(|_| DevError::BrokerDeadline)?;
        match reply {
            Ok(reply) => {
                return match serde_json::from_slice::<GetAccountLimitsResponse>(&reply.payload)
                    .map_err(DevError::InvalidBrokerResponse)?
                {
                    GetAccountLimitsResponse::Limits { .. } => Ok(()),
                    GetAccountLimitsResponse::Denied(_) => Err(DevError::BrokerRefused),
                };
            }
            Err(error) if error.kind() == RequestErrorKind::NoResponders => {
                tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + Duration::from_millis(100))).await;
            }
            Err(error) => return Err(DevError::BrokerRequest(error)),
        }
    }
}
