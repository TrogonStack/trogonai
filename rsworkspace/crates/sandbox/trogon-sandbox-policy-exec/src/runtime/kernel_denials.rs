//! Drains the kernel's filesystem denial stream into the shared violation store.

use trogon_sandbox_egress_proxy::{CommandId, ViolationFilter, ViolationStore};

use crate::constants::VIOLATION_STREAM_WAIT;

#[cfg(target_os = "linux")]
use trogon_sandbox_subprocess_worker::fs_violation::FsDenial;

#[cfg(target_os = "linux")]
pub(crate) struct KernelDenials {
    task: tokio::task::JoinHandle<()>,
}

#[cfg(target_os = "linux")]
impl KernelDenials {
    pub(crate) fn record(
        mut receiver: tokio::sync::mpsc::Receiver<FsDenial>,
        store: ViolationStore,
        command: CommandId,
        filter: ViolationFilter,
    ) -> Self {
        let task = tokio::spawn(async move {
            while let Some(denial) = receiver.recv().await {
                let operation = format!("{:?}", denial.operation).to_lowercase();
                let line = format!("deny {operation} {}", denial.path);
                if !filter.ignores(&line) {
                    store.add(&line, Some(command.clone()));
                }
            }
        });
        Self { task }
    }

    pub(crate) async fn finish(self) {
        let mut task = self.task;
        if tokio::time::timeout(VIOLATION_STREAM_WAIT, &mut task).await.is_err() {
            task.abort();
        }
    }
}

#[cfg(target_os = "macos")]
use trogon_sandbox_subprocess_worker::isolation::seatbelt::{SeatbeltViolation, ViolationKind};

#[cfg(target_os = "macos")]
pub(crate) struct KernelDenials {
    task: tokio::task::JoinHandle<()>,
}

#[cfg(target_os = "macos")]
impl KernelDenials {
    pub(crate) fn record(
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<SeatbeltViolation>,
        store: ViolationStore,
        command: CommandId,
        filter: ViolationFilter,
    ) -> Self {
        let task = tokio::spawn(async move {
            while let Some(violation) = receiver.recv().await {
                let line = describe_denial(&violation);
                if !filter.ignores(&line) {
                    store.add(&line, Some(command.clone()));
                }
            }
        });
        Self { task }
    }

    pub(crate) async fn finish(self) {
        let mut task = self.task;
        if tokio::time::timeout(VIOLATION_STREAM_WAIT, &mut task).await.is_err() {
            task.abort();
        }
    }
}

#[cfg(target_os = "macos")]
fn describe_denial(violation: &SeatbeltViolation) -> String {
    match &violation.kind {
        ViolationKind::FileRead { path } | ViolationKind::FileWrite { path } => {
            format!("deny {} {path}", violation.operation)
        }
        ViolationKind::NetworkConnect { ip: Some(ip), port } => {
            format!("deny {} {ip}:{port}", violation.operation)
        }
        ViolationKind::NetworkConnect { ip: None, port } => {
            format!("deny {} port {port}", violation.operation)
        }
        ViolationKind::Other { detail } => format!("deny {} {detail}", violation.operation),
    }
}
