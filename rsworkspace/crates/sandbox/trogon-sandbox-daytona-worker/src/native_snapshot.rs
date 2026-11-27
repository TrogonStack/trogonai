//! Taking a native Daytona snapshot of a running sandbox.

use tokio::time::Instant;
use trogon_sandbox_contract::{
    NativeSnapshotId, NativeSnapshotReceipt, NativeSnapshotRef, ProviderKind, SnapshotExportError,
    SnapshotExportOutcome,
};

use crate::{DaytonaWorkerProvider, SnapshotState};

fn transient(message: String) -> SnapshotExportOutcome {
    SnapshotExportOutcome::Err(SnapshotExportError::Transient { message })
}

impl DaytonaWorkerProvider {
    /// Takes a native snapshot of `sandbox` under `name` and waits until it is usable.
    ///
    /// Daytona answers the capture request before the snapshot is built, so only a snapshot that reports active is a
    /// receipt. A build that fails is permanent; running out of time is transient, because the snapshot may still
    /// finish and custody keeps it until the source is cleaned up.
    pub(crate) async fn native_snapshot(&self, sandbox: &str, name: &str) -> SnapshotExportOutcome {
        let Some(custody) = &self.snapshot_custody else {
            return transient("native snapshot capture requires durable custody".to_owned());
        };
        let gate = custody.gate(sandbox);
        let _held = gate.lock().await;
        if let Err(error) = custody.begin(&self.api, sandbox, name).await {
            return transient(format!("native snapshot custody admission failed: {error}"));
        }
        let api = match custody.scoped_api(&self.api, sandbox).await {
            Ok(api) => api,
            Err(error) => return transient(format!("native snapshot account binding failed: {error}")),
        };
        match api.snapshot_sandbox(sandbox, name).await {
            Ok(answer) => {
                if let Err(error) = custody.acknowledge(sandbox, name, &answer).await {
                    return transient(format!("native snapshot create binding failed: {error}"));
                }
            }
            Err(error) => return transient(format!("Daytona create snapshot: {error}")),
        }
        let wait = self.snapshot_wait();
        let deadline = Instant::now() + wait.timeout;
        let mut last;
        loop {
            match api.find_snapshot(name).await {
                Ok(Some(snapshot)) => {
                    let active = snapshot.state == SnapshotState::Active;
                    if let Err(error) = custody.observe(sandbox, name, &snapshot, active).await {
                        return transient(format!("native snapshot custody binding failed: {error}"));
                    }
                    if active {
                        return match NativeSnapshotId::new(snapshot.name) {
                            Ok(id) => SnapshotExportOutcome::Native(NativeSnapshotReceipt {
                                reference: NativeSnapshotRef {
                                    provider: ProviderKind::Daytona,
                                    id,
                                    expires_at: None,
                                },
                                bytes: snapshot.size,
                            }),
                            Err(error) => transient(format!("Daytona named the snapshot with an unusable id: {error}")),
                        };
                    }
                    if snapshot.state.is_failed() {
                        return SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
                            message: format!(
                                "Daytona snapshot {name} failed: {}",
                                snapshot.error_reason.unwrap_or_default()
                            ),
                        });
                    }
                    last = format!("snapshot state {:?}", snapshot.state);
                }
                Ok(None) => last = "the snapshot is not visible yet".to_owned(),
                Err(error) => last = error.to_string(),
            }
            if Instant::now() >= deadline {
                return transient(format!("Daytona snapshot {name} not active in time: {last}"));
            }
            tokio::time::sleep(wait.poll_interval).await;
        }
    }
}
