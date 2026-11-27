use std::future::Future;
use std::time::Duration;

use trogon_sandbox_providers::PodExecOutput;

use crate::constants::COMMAND_COMPLETION_SETTLE;
use crate::execution_tracker::Completion;
use crate::{ExecutionTracker, K8sWorkerError};

/// The `KEY=VALUE` entry that tags every process of an execution, so the
/// whole tree can be found again by scanning the pod's process environments.
pub(crate) fn execution_marker(execution_id: &str) -> String {
    format!("{}={execution_id}", trogon_sandbox_contract::CommandEnv::RESERVED_KEY)
}

fn timed_out_result(mut output: PodExecOutput, timeout: Duration) -> PodExecOutput {
    output.exit_code = -1;
    output.timed_out = true;
    if output.stderr.is_empty() {
        output.stderr = format!("command timed out after {}s", timeout.as_secs()).into_bytes();
    }
    output
}

/// An accepted signal is not process-exit evidence, so the original
/// completion must survive signaling and remain owned while uncertain.
pub(crate) fn bound_completion<K, KF>(
    mut completion: Completion,
    timeout: Duration,
    kill: K,
    retained: Option<(ExecutionTracker, String)>,
) -> Completion
where
    K: FnOnce() -> KF + Send + 'static,
    KF: Future<Output = Result<(), K8sWorkerError>> + Send + 'static,
{
    Box::pin(async move {
        if let Ok(result) = tokio::time::timeout(timeout, &mut completion).await {
            return result;
        }
        if let Err(error) = kill().await {
            if !mark_uncertain(&retained, error.to_string()).await {
                return Err(error);
            }
            return completion.await;
        }
        match tokio::time::timeout(COMMAND_COMPLETION_SETTLE, &mut completion).await {
            Ok(Ok(output)) => Ok(timed_out_result(output, timeout)),
            Ok(Err(error)) => {
                mark_uncertain(&retained, error.to_string()).await;
                Err(error)
            }
            Err(_) => {
                let error = K8sWorkerError::CompletionUnconfirmed;
                if !mark_uncertain(&retained, error.to_string()).await {
                    return Err(error);
                }
                completion.await.map(|output| timed_out_result(output, timeout))
            }
        }
    })
}

async fn mark_uncertain(retained: &Option<(ExecutionTracker, String)>, message: String) -> bool {
    match retained {
        Some((tracker, execution_id)) => tracker.mark_uncertain(execution_id, message).await,
        None => false,
    }
}

#[cfg(test)]
mod tests;
