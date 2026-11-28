use super::*;

use crate::command_logs::{CommandLogError, CommandLogStore};
use crate::log_page::LogPage;
use trogon_sandbox_contract::{ReadLogsRequest, StopCommandRequest, WaitReadyRequest, WaitSandboxReadyRequest};

/// A log store that remembers the bounds the broker asks it for.
#[derive(Default)]
struct BoundsSpy {
    max_bytes: Mutex<Vec<u64>>,
    waits: Mutex<Vec<Duration>>,
}

#[async_trait::async_trait]
impl CommandLogStore for BoundsSpy {
    async fn read(
        &self,
        _lease_id: &LeaseId,
        _execution_id: &ExecutionId,
        _after_seq: u64,
        max_bytes: u64,
    ) -> Result<LogPage, CommandLogError> {
        self.max_bytes
            .lock()
            .map_err(|_| CommandLogError::InvalidExecution)?
            .push(max_bytes);
        Ok(LogPage::default())
    }

    async fn wait_for_chunks(
        &self,
        _lease_id: &LeaseId,
        _execution_id: &ExecutionId,
        _after_seq: u64,
        wait: Duration,
    ) -> Result<(), CommandLogError> {
        self.waits
            .lock()
            .map_err(|_| CommandLogError::InvalidExecution)?
            .push(wait);
        Ok(())
    }

    async fn purge_lease(&self, _lease_id: &LeaseId) -> Result<(), CommandLogError> {
        Ok(())
    }
}

async fn capturing_core(reply: Bytes) -> anyhow::Result<(BrokerCore, Arc<CapturingBus>, LeaseId)> {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let mut core = make_core(mock_provider_response()?)?;
    let bus = Arc::new(CapturingBus::new(reply));
    core.bus = bus.clone();
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    Ok((core, bus, lease_id))
}

fn forwarded<T: serde::de::DeserializeOwned>(bus: &CapturingBus) -> anyhow::Result<T> {
    let payload = locked(&bus.last_payload)?
        .clone()
        .context("no payload was dispatched")?;
    Ok(serde_json::from_slice(&payload)?)
}

#[tokio::test]
async fn stop_forwards_a_grace_period_capped_at_the_maximum() -> TestResult {
    let reply = Bytes::from(serde_json::to_vec(&StopCommandResponse::Unsupported)?);
    let (core, bus, lease_id) = capturing_core(reply).await?;
    let mut request = stop_request(&lease_id);
    request.grace_seconds = u32::MAX;
    core.handle_stop_command(request).await;
    let sent: StopCommandRequest = forwarded(&bus)?;
    assert_eq!(sent.grace_seconds, StopCommandRequest::MAX_GRACE_SECONDS);

    let mut request = stop_request(&lease_id);
    request.grace_seconds = 7;
    core.handle_stop_command(request).await;
    let sent: StopCommandRequest = forwarded(&bus)?;
    assert_eq!(sent.grace_seconds, 7);
    Ok(())
}

#[tokio::test]
async fn restart_forwards_a_grace_period_capped_at_the_maximum() -> TestResult {
    let reply = Bytes::from(serde_json::to_vec(&RestartCommandResponse::Unsupported)?);
    let (core, bus, lease_id) = capturing_core(reply).await?;
    let mut request = restart_request(&lease_id);
    request.grace_seconds = u32::MAX;
    core.handle_restart_command(request).await;
    let sent: RestartCommandRequest = forwarded(&bus)?;
    assert_eq!(sent.grace_seconds, StopCommandRequest::MAX_GRACE_SECONDS);
    Ok(())
}

#[tokio::test]
async fn wait_ready_forwards_a_timeout_capped_at_the_maximum() -> TestResult {
    let reply = Bytes::from(serde_json::to_vec(&WaitReadyResponse::Ready)?);
    let (core, bus, lease_id) = capturing_core(reply).await?;
    core.handle_wait_ready(WaitReadyRequest {
        lease_id: lease_id.clone(),
        execution_id: ExecutionId::new("exec-1"),
        timeout_seconds: u32::MAX,
    })
    .await;
    let sent: WaitReadyRequest = forwarded(&bus)?;
    assert_eq!(sent.timeout_seconds, WaitReadyRequest::MAX_TIMEOUT_SECONDS);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn wait_sandbox_ready_gives_up_after_the_maximum_timeout() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.status = LeaseStatus::Pending;
    core.active_leases.put(&record).await?;

    let mut request = sandbox_ready_request(lease_id, vec![])?;
    request.timeout_seconds = u32::MAX;
    let started = tokio::time::Instant::now();
    let response = core.handle_wait_sandbox_ready(request).await;
    let waited = started.elapsed();
    assert!(
        matches!(response, WaitSandboxReadyResponse::NotReady { .. }),
        "{response:?}"
    );
    let max = Duration::from_secs(u64::from(WaitSandboxReadyRequest::MAX_TIMEOUT_SECONDS));
    assert!(waited >= max, "{waited:?}");
    assert!(waited < max + Duration::from_secs(60), "{waited:?}");
    Ok(())
}

#[tokio::test]
async fn read_logs_bounds_the_page_size() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let spy = Arc::new(BoundsSpy::default());
    let core = make_core(mock_provider_response()?)?.with_command_logs(spy.clone());
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    for requested in [None, Some(u64::MAX), Some(0), Some(1_000)] {
        let mut request = read_logs_request(&lease_id, "exec-1", 0);
        request.max_bytes = requested;
        core.handle_read_logs(request).await;
    }
    assert_eq!(
        *locked(&spy.max_bytes)?,
        [ReadLogsRequest::DEFAULT_BYTES, ReadLogsRequest::MAX_BYTES, 1, 1_000]
    );
    Ok(())
}

#[tokio::test]
async fn read_logs_bounds_the_long_poll() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let spy = Arc::new(BoundsSpy::default());
    let core = make_core(mock_provider_response()?)?.with_command_logs(spy.clone());
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let mut request = read_logs_request(&lease_id, "exec-1", 0);
    core.handle_read_logs(request.clone()).await;
    assert!(locked(&spy.waits)?.is_empty(), "no wait means no long poll");

    request.wait_ms = Some(u64::MAX);
    core.handle_read_logs(request).await;
    assert_eq!(
        *locked(&spy.waits)?,
        [Duration::from_millis(ReadLogsRequest::MAX_WAIT_MS)]
    );
    Ok(())
}
