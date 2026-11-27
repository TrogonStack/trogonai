use std::future::Future;
use std::time::Instant;

use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{GetSandboxDomainRequest, GetSandboxDomainResponse, PortAccess};

use crate::DispatchError;
use crate::constants::{PROBE_CONNECT_TIMEOUT, PROBE_FIRST_PAUSE, PROBE_MAX_PAUSE, PROBE_TIMEOUT};
use crate::upstream::{access_headers, upstream_base};

/// True when something answers HTTP on the port. Any status counts except the gateway-style 502 to 504 a fronting
/// proxy returns while nothing listens.
async fn port_answers(client: &reqwest::Client, endpoint: &str, access: Option<&PortAccess>) -> bool {
    let url = format!("{}/", upstream_base(endpoint, access));
    match client.get(url).headers(access_headers(access)).send().await {
        Ok(response) => !matches!(response.status().as_u16(), 502..=504),
        Err(_) => false,
    }
}

/// Holds an `Available` reply until the port answers, re-resolving it through `resolve` on every attempt so a
/// restarted server on a new endpoint is found. Returns `NotReady` once `wait_ms`, clamped to
/// [`GetSandboxDomainRequest::MAX_WAIT_MS`], has passed. Other outcomes pass through at once.
pub(crate) async fn wait_until_answering<F, Fut>(
    first: GetSandboxDomainResponse,
    wait_ms: u64,
    mut resolve: F,
) -> Result<GetSandboxDomainResponse, DispatchError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<GetSandboxDomainResponse, ClientError>>,
{
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(PROBE_CONNECT_TIMEOUT)
        .timeout(PROBE_TIMEOUT)
        .build()?;
    let wait = std::time::Duration::from_millis(wait_ms.min(GetSandboxDomainRequest::MAX_WAIT_MS));
    let started = Instant::now();
    let mut current = first;
    let mut pause = PROBE_FIRST_PAUSE;
    loop {
        let GetSandboxDomainResponse::Available { endpoint, access, .. } = &current else {
            return Ok(current);
        };
        if port_answers(&client, endpoint, access.as_ref()).await {
            return Ok(current);
        }
        let elapsed = started.elapsed();
        if elapsed >= wait {
            let waited_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
            return Ok(GetSandboxDomainResponse::NotReady { waited_ms });
        }
        tokio::time::sleep(pause.min(wait - elapsed)).await;
        pause = (pause * 2).min(PROBE_MAX_PAUSE);
        current = resolve().await?;
    }
}
