use std::net::SocketAddr;

use trogon_sandbox_contract::NetworkPolicy;

use crate::check_failure::ensure;
use crate::constants::{NC_PROBE, PYTHON_PROBE, TOOL_PREFLIGHT};
use crate::encoding::text;
use crate::held::Held;
use crate::run_fields::RunFields;
use crate::{CheckFailureError, CheckFuture, Target, outcomes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EgressProbeTool {
    Nc,
    Python3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectProbeResult {
    Connected,
    Denied,
}

async fn probe_token(held: &Held<'_>, command: &[&str]) -> Result<String, CheckFailureError> {
    let response = held
        .run_with(
            command,
            RunFields {
                timeout_seconds: Some(20),
                max_output_bytes: Some(4096),
                ..RunFields::default()
            },
        )
        .await?;
    let output = outcomes::completed(response)?;
    let clean = output.exit_code == 0 && !output.timed_out.is_timed_out() && !output.truncated.is_truncated();
    let complete = clean && output.stderr.is_empty() && output.stdout.len() <= 4096;
    ensure(complete, "a probe that exits 0 with complete diagnostics", &output)?;
    let token = text(&output.stdout);
    Ok(token.strip_suffix('\n').unwrap_or(&token).to_owned())
}

/// The probe tool the image carries, from the marker its preflight printed.
pub(crate) fn probe_tool_from(marker: &str) -> Result<EgressProbeTool, CheckFailureError> {
    [("NC", EgressProbeTool::Nc), ("PYTHON3", EgressProbeTool::Python3)]
        .into_iter()
        .find_map(|(known, tool)| (known == marker).then_some(tool))
        .ok_or_else(|| CheckFailureError::unexpected("an image with nc or a Python3 socket probe", &marker))
}

/// The connection result from the marker a probe printed.
pub(crate) fn connect_result_from(marker: &str) -> Result<ConnectProbeResult, CheckFailureError> {
    [
        ("CONNECTED", ConnectProbeResult::Connected),
        ("DENIED", ConnectProbeResult::Denied),
    ]
    .into_iter()
    .find_map(|(known, result)| (known == marker).then_some(result))
    .ok_or_else(|| CheckFailureError::unexpected("CONNECTED or DENIED", &marker))
}

async fn probe_tool(control: &Held<'_>) -> Result<EgressProbeTool, CheckFailureError> {
    probe_tool_from(&probe_token(control, &["sh", "-c", TOOL_PREFLIGHT]).await?)
}

async fn connect(
    held: &Held<'_>,
    tool: EgressProbeTool,
    endpoint: SocketAddr,
) -> Result<ConnectProbeResult, CheckFailureError> {
    let host = endpoint.ip().to_string();
    let port = endpoint.port().to_string();
    let token = match tool {
        EgressProbeTool::Nc => probe_token(held, &["sh", "-c", NC_PROBE, "sandbox-egress-probe", &host, &port]).await?,
        EgressProbeTool::Python3 => probe_token(held, &["python3", "-c", PYTHON_PROBE, &host, &port]).await?,
    };
    connect_result_from(&token)
}

async fn positive_control(
    control: &Held<'_>,
    tool: EgressProbeTool,
    endpoint: SocketAddr,
    phase: &str,
) -> Result<(), CheckFailureError> {
    let result = connect(control, tool, endpoint).await?;
    ensure(
        result == ConnectProbeResult::Connected,
        format!("the unrestricted {phase} control to connect to {endpoint}"),
        &result,
    )
}

/// Independent healthy connections prevent endpoint or probe failures from proving denial.
pub(crate) fn denied_connect_fails(target: &Target) -> CheckFuture<'_> {
    Box::pin(async move {
        let endpoint = target.egress_probe_address;
        let concrete = !endpoint.ip().is_unspecified() && endpoint.port() != 0;
        ensure(concrete, "a concrete egress probe endpoint", &endpoint)?;
        let control =
            Held::acquire_request(target, target.lease_request_with_network(NetworkPolicy::AllowAll)?).await?;
        let result = restricted_connection_fails(target, &control, endpoint).await;
        control.release().await;
        result
    })
}

async fn restricted_connection_fails(
    target: &Target,
    control: &Held<'_>,
    endpoint: SocketAddr,
) -> Result<(), CheckFailureError> {
    let tool = probe_tool(control).await?;
    positive_control(control, tool, endpoint, "before").await?;
    let denied = Held::acquire_request(target, target.lease_request_with_network(NetworkPolicy::DenyAll)?).await?;
    let restricted = connect(&denied, tool, endpoint).await;
    denied.release().await;
    let after = positive_control(control, tool, endpoint, "after").await;
    let restricted = restricted?;
    after?;
    ensure(
        restricted == ConnectProbeResult::Denied,
        format!("a lease under DenyAll not to open a connection to {endpoint}"),
        &restricted,
    )
}
