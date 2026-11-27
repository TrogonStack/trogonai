use k8s_openapi::api::core::v1::Pod;

use crate::k8s_ops::constants::{AGENT_CONTAINER_NAME, FATAL_WAITING_REASONS};

/// Where a Job's pod is on the way to serving exec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JobPodReadiness {
    /// The `agent` container is running.
    Ready,
    Starting,
    Failed(String),
}

pub(crate) fn job_pod_readiness(pod: &Pod) -> JobPodReadiness {
    let Some(status) = pod.status.as_ref() else {
        return JobPodReadiness::Starting;
    };
    if let Some(phase @ ("Failed" | "Succeeded")) = status.phase.as_deref() {
        return JobPodReadiness::Failed(format!("pod ended in phase {phase}"));
    }
    let containers = status
        .init_container_statuses
        .iter()
        .flatten()
        .chain(status.container_statuses.iter().flatten());
    for container in containers {
        let Some(waiting) = container.state.as_ref().and_then(|s| s.waiting.as_ref()) else {
            continue;
        };
        let reason = waiting.reason.as_deref().unwrap_or_default();
        if FATAL_WAITING_REASONS.contains(&reason) {
            let message = waiting.message.as_deref().unwrap_or_default();
            return JobPodReadiness::Failed(format!(
                "container {} cannot start: {reason}: {message}",
                container.name
            ));
        }
    }
    let agent_running = status
        .container_statuses
        .iter()
        .flatten()
        .any(|c| c.name == AGENT_CONTAINER_NAME && c.state.as_ref().is_some_and(|s| s.running.is_some()));
    if pod_is_running(pod) && agent_running {
        JobPodReadiness::Ready
    } else {
        JobPodReadiness::Starting
    }
}

pub fn pod_is_running_public(pod: &Pod) -> bool {
    pod.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running")
}

pub(crate) fn pod_is_running(pod: &Pod) -> bool {
    pod_is_running_public(pod)
}
