use std::collections::BTreeMap;
use std::time::Duration;

use k8s_openapi::api::batch::v1::{Job, JobCondition, JobSpec, JobStatus};
use k8s_openapi::api::core::v1::{
    ContainerState, ContainerStateWaiting, ContainerStatus, Pod, PodSpec, PodStatus, SecretVolumeSource, Volume,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Status, StatusCause, StatusDetails, Time};
use k8s_openapi::jiff::Timestamp;
use kube::api::PropagationPolicy;

use crate::k8s_ops::constants::{
    KEEP_ALIVE_SCRIPT, LABEL_LEASE_DEADLINE, RESTORE_VOLUME_NAME, RUNTIME_TMP_MAX_MIB, RUNTIME_TMP_VOLUME,
    SECCOMP_PROFILE_TYPE, WORKSPACE_FS_GROUP,
};
use crate::k8s_ops::{
    ApiServerClock, JobPhase, JobPodReadiness, clone_secret_name, container_resources, exit_code_from_status,
    hardened_pod_security_context, job_deadline_seconds, job_delete_params, job_is_finished, job_outlived_deadline,
    job_outlived_deadline_by, job_phase_from_k8s, job_pod_readiness, keep_alive_command, lease_job_secret_names,
    lease_request_secret_name, parse_http_date, pod_age_seconds, pod_is_running, pod_is_running_public,
    restore_secret_name, runtime_tmp_volume, s3_volume_secret_name, warm_pod_outlived_deadline,
    warm_pod_outlived_deadline_by, warm_pod_restore_secret_name,
};

fn at(value: &str) -> Result<Timestamp, k8s_openapi::jiff::Error> {
    value.parse()
}

fn clock() -> Result<ApiServerClock, k8s_openapi::jiff::Error> {
    at("2024-01-01T00:10:00Z")
}

fn lease_job(age_seconds: i64, deadline: i64) -> Result<Job, Box<dyn std::error::Error>> {
    let created = clock()?.checked_sub(k8s_openapi::jiff::SignedDuration::from_secs(age_seconds))?;
    Ok(Job {
        metadata: ObjectMeta {
            creation_timestamp: Some(Time(created)),
            ..Default::default()
        },
        spec: Some(JobSpec {
            active_deadline_seconds: Some(deadline),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn with_conditions(conditions: &[(&str, &str, Option<&str>)], active: Option<i32>) -> Job {
    Job {
        status: Some(JobStatus {
            active,
            conditions: Some(
                conditions
                    .iter()
                    .map(|(type_, status, message)| JobCondition {
                        type_: (*type_).to_string(),
                        status: (*status).to_string(),
                        message: message.map(str::to_string),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn job_pod(phase: &str, agent: ContainerState) -> Pod {
    Pod {
        status: Some(PodStatus {
            phase: Some(phase.to_string()),
            container_statuses: Some(vec![ContainerStatus {
                name: "agent".to_string(),
                state: Some(agent),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn running() -> ContainerState {
    ContainerState {
        running: Some(Default::default()),
        ..Default::default()
    }
}

fn waiting(reason: &str) -> ContainerState {
    ContainerState {
        waiting: Some(ContainerStateWaiting {
            reason: Some(reason.to_string()),
            message: Some("detail".to_string()),
        }),
        ..Default::default()
    }
}

fn pod_with_deadline(deadline: Option<&str>) -> Pod {
    let mut labels = BTreeMap::new();
    if let Some(deadline) = deadline {
        labels.insert(LABEL_LEASE_DEADLINE.to_string(), deadline.to_string());
    }
    Pod {
        metadata: ObjectMeta {
            labels: Some(labels),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn pod_age_is_measured_from_the_apiserver_creation_timestamp() -> Result<(), Box<dyn std::error::Error>> {
    let pod = Pod {
        metadata: ObjectMeta {
            creation_timestamp: Some(Time(at("2024-01-01T00:00:00Z")?)),
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(pod_age_seconds(&pod, clock()?), Some(600));
    Ok(())
}

#[test]
fn pod_with_no_creation_timestamp_has_no_age() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(pod_age_seconds(&Pod::default(), clock()?), None);
    Ok(())
}

#[test]
fn http_date_header_parses_as_the_apiserver_clock() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_http_date("Tue, 15 Nov 1994 08:12:31 GMT"),
        Some(at("1994-11-15T08:12:31Z")?)
    );
    Ok(())
}

#[test]
fn an_http_date_with_a_numeric_offset_parses_too() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_http_date(" Tue, 15 Nov 1994 08:12:31 +0000 "),
        Some(at("1994-11-15T08:12:31Z")?)
    );
    Ok(())
}

/// An unparseable header reads as "no clock", which callers turn into a
/// skipped sweep rather than a fall back to local time.
#[test]
fn unparseable_http_date_yields_no_clock() {
    assert_eq!(parse_http_date("not a date"), None);
    assert_eq!(parse_http_date(""), None);
}

#[test]
fn a_job_expires_once_it_outlives_its_deadline_on_the_apiserver_clock() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!job_outlived_deadline(&lease_job(299, 300)?, clock()?));
    assert!(job_outlived_deadline(&lease_job(300, 300)?, clock()?));
    let mut no_deadline = lease_job(10_000, 300)?;
    no_deadline.spec = None;
    assert!(!job_outlived_deadline(&no_deadline, clock()?));
    Ok(())
}

#[test]
fn a_job_without_a_creation_timestamp_never_expires() -> Result<(), Box<dyn std::error::Error>> {
    let mut job = lease_job(10_000, 300)?;
    job.metadata.creation_timestamp = None;
    assert!(!job_outlived_deadline(&job, clock()?));
    Ok(())
}

#[test]
fn a_grace_period_extends_a_jobs_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let job = lease_job(350, 300)?;
    assert!(job_outlived_deadline_by(&job, clock()?, 0));
    assert!(!job_outlived_deadline_by(&job, clock()?, 100));
    assert!(job_outlived_deadline_by(&job, clock()?, 50));
    Ok(())
}

#[test]
fn a_claimed_warm_pod_expires_at_its_deadline_label() -> Result<(), Box<dyn std::error::Error>> {
    let now = clock()?;
    let deadline = now.as_second();
    assert!(warm_pod_outlived_deadline(
        &pod_with_deadline(Some(&deadline.to_string())),
        now
    ));
    assert!(!warm_pod_outlived_deadline(
        &pod_with_deadline(Some(&(deadline + 1).to_string())),
        now
    ));
    assert!(!warm_pod_outlived_deadline(&pod_with_deadline(Some("soon")), now));
    assert!(!warm_pod_outlived_deadline(&pod_with_deadline(None), now));
    assert!(!warm_pod_outlived_deadline(&Pod::default(), now));
    assert!(warm_pod_outlived_deadline_by(
        &pod_with_deadline(Some(&(deadline - 5).to_string())),
        now,
        5
    ));
    assert!(!warm_pod_outlived_deadline_by(
        &pod_with_deadline(Some(&(deadline - 5).to_string())),
        now,
        6
    ));
    Ok(())
}

#[test]
fn a_job_is_finished_only_by_a_true_terminal_condition() {
    assert!(job_is_finished(&with_conditions(&[("Complete", "True", None)], None)));
    assert!(job_is_finished(&with_conditions(&[("Failed", "True", None)], None)));
    assert!(!job_is_finished(&with_conditions(&[("Suspended", "True", None)], None)));
    assert!(!job_is_finished(&with_conditions(&[("Complete", "False", None)], None)));
    assert!(!job_is_finished(&Job::default()));
}

#[test]
fn a_zero_timeout_has_no_job_deadline() {
    assert_eq!(job_deadline_seconds(Duration::ZERO), None);
    assert_eq!(job_deadline_seconds(Duration::from_secs(300)), Some(300));
    assert_eq!(job_deadline_seconds(Duration::MAX), Some(i64::MAX));
}

#[test]
fn a_job_without_status_is_pending() {
    assert_eq!(job_phase_from_k8s(&Job::default()), JobPhase::Pending);
}

#[test]
fn a_complete_job_succeeded() {
    assert_eq!(
        job_phase_from_k8s(&with_conditions(&[("Complete", "true", None)], Some(0))),
        JobPhase::Succeeded
    );
}

#[test]
fn a_failed_job_reports_its_condition_message() {
    assert_eq!(
        job_phase_from_k8s(&with_conditions(&[("Failed", "True", Some("deadline exceeded"))], None)),
        JobPhase::Failed {
            reason: "deadline exceeded".to_string()
        }
    );
    assert_eq!(
        job_phase_from_k8s(&with_conditions(&[("Failed", "True", None)], None)),
        JobPhase::Failed {
            reason: "job condition Failed=True".to_string()
        }
    );
}

#[test]
fn a_job_with_active_pods_is_running_and_otherwise_pending() {
    assert_eq!(
        job_phase_from_k8s(&with_conditions(&[("Complete", "False", None)], Some(1))),
        JobPhase::Running
    );
    assert_eq!(job_phase_from_k8s(&with_conditions(&[], Some(0))), JobPhase::Pending);
    assert_eq!(job_phase_from_k8s(&with_conditions(&[], None)), JobPhase::Pending);
}

#[test]
fn a_job_pod_is_ready_once_its_agent_container_runs() {
    assert_eq!(
        job_pod_readiness(&job_pod("Running", running())),
        JobPodReadiness::Ready
    );
    assert_eq!(
        job_pod_readiness(&job_pod("Pending", waiting("ContainerCreating"))),
        JobPodReadiness::Starting
    );
    assert_eq!(
        job_pod_readiness(&job_pod("Running", ContainerState::default())),
        JobPodReadiness::Starting
    );
    assert_eq!(job_pod_readiness(&Pod::default()), JobPodReadiness::Starting);
}

#[test]
fn a_job_pod_that_cannot_start_is_failed() {
    let readiness = job_pod_readiness(&job_pod("Pending", waiting("CreateContainerConfigError")));
    assert!(
        matches!(&readiness, JobPodReadiness::Failed(reason) if reason.contains("CreateContainerConfigError")),
        "{readiness:?}"
    );
    assert!(matches!(
        job_pod_readiness(&job_pod("Succeeded", ContainerState::default())),
        JobPodReadiness::Failed(reason) if reason == "pod ended in phase Succeeded"
    ));
}

#[test]
fn a_fatal_init_container_failure_fails_the_pod() {
    let mut pod = job_pod("Pending", ContainerState::default());
    if let Some(status) = pod.status.as_mut() {
        status.init_container_statuses = Some(vec![ContainerStatus {
            name: "git-init".to_string(),
            state: Some(waiting("ImagePullBackOff")),
            ..Default::default()
        }]);
    }
    assert!(matches!(
        job_pod_readiness(&pod),
        JobPodReadiness::Failed(reason) if reason.contains("git-init")
    ));
}

#[test]
fn a_waiting_container_without_a_reason_is_not_fatal() {
    let state = ContainerState {
        waiting: Some(ContainerStateWaiting::default()),
        ..Default::default()
    };
    assert_eq!(job_pod_readiness(&job_pod("Pending", state)), JobPodReadiness::Starting);
}

#[test]
fn only_a_running_phase_counts_as_running() {
    assert!(pod_is_running(&job_pod("Running", running())));
    assert!(pod_is_running_public(&job_pod("Running", running())));
    assert!(!pod_is_running(&job_pod("Pending", running())));
    assert!(!pod_is_running(&Pod::default()));
}

fn status_with_exit_code(message: &str) -> Status {
    Status {
        status: Some("Failure".to_string()),
        details: Some(StatusDetails {
            causes: Some(vec![
                StatusCause {
                    reason: Some("Other".to_string()),
                    message: Some("9".to_string()),
                    ..Default::default()
                },
                StatusCause {
                    reason: Some("ExitCode".to_string()),
                    message: Some(message.to_string()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn a_missing_or_successful_status_is_exit_code_zero() {
    assert_eq!(exit_code_from_status(None), 0);
    let success = Status {
        status: Some("Success".to_string()),
        ..Default::default()
    };
    assert_eq!(exit_code_from_status(Some(&success)), 0);
}

#[test]
fn a_failure_carries_its_exit_code_cause() {
    assert_eq!(exit_code_from_status(Some(&status_with_exit_code("42"))), 42);
}

#[test]
fn a_failure_without_a_numeric_exit_code_is_exit_code_one() {
    assert_eq!(exit_code_from_status(Some(&status_with_exit_code("many"))), 1);
    let bare = Status {
        status: Some("Failure".to_string()),
        ..Default::default()
    };
    assert_eq!(exit_code_from_status(Some(&bare)), 1);
}

#[test]
fn the_lease_request_secret_is_swept_with_the_lease() {
    assert!(
        lease_job_secret_names("rs-job").contains(&lease_request_secret_name("rs-job")),
        "teardown must delete the stored request"
    );
    assert_eq!(lease_request_secret_name("rs-job"), "rs-job-lease-req");
}

#[test]
fn every_launch_secret_name_derives_from_the_job_name() {
    assert_eq!(
        lease_job_secret_names("j"),
        vec![
            clone_secret_name("j"),
            restore_secret_name("j"),
            s3_volume_secret_name("j"),
            lease_request_secret_name("j"),
        ]
    );
    assert_eq!(clone_secret_name("j"), "j-clone-creds");
    assert_eq!(restore_secret_name("j"), "j-restore");
    assert_eq!(s3_volume_secret_name("j"), "j-s3-volume");
}

#[test]
fn a_pod_without_a_restore_volume_has_no_restore_secret() {
    assert_eq!(warm_pod_restore_secret_name(&Pod::default()), None);
    let other_volume = Pod {
        spec: Some(PodSpec {
            volumes: Some(vec![Volume {
                name: "other".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(warm_pod_restore_secret_name(&other_volume), None);
}

#[test]
fn a_warm_pod_names_the_restore_secret_it_mounts() {
    let pod = Pod {
        spec: Some(PodSpec {
            volumes: Some(vec![Volume {
                name: RESTORE_VOLUME_NAME.to_string(),
                secret: Some(SecretVolumeSource {
                    secret_name: Some("restore-xyz".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(warm_pod_restore_secret_name(&pod).as_deref(), Some("restore-xyz"));
}

#[test]
fn deleting_a_job_also_deletes_its_pods() {
    let params = job_delete_params();
    assert!(matches!(params.propagation_policy, Some(PropagationPolicy::Background)));
    assert_eq!(params.grace_period_seconds, Some(0));
}

#[test]
fn zero_memory_omits_the_memory_quantity() {
    let resources = container_resources(0.5, 0);
    assert_eq!(resources.get("cpu").map(|q| q.0.as_str()), Some("0.5"));
    assert!(!resources.contains_key("memory"));
    assert_eq!(
        container_resources(1.0, 256).get("memory").map(|q| q.0.as_str()),
        Some("256Mi")
    );
}

#[test]
fn the_pod_security_context_pins_seccomp_and_the_workspace_group() {
    let context = hardened_pod_security_context();
    assert_eq!(
        context.seccomp_profile.map(|profile| profile.type_),
        Some(SECCOMP_PROFILE_TYPE.to_string())
    );
    assert_eq!(context.fs_group, Some(WORKSPACE_FS_GROUP));
}

#[test]
fn the_keep_alive_command_runs_the_script_in_a_shell() {
    assert_eq!(
        keep_alive_command(),
        vec!["/bin/sh".to_string(), "-c".to_string(), KEEP_ALIVE_SCRIPT.to_string()]
    );
}

#[test]
fn the_runtime_tmp_volume_is_memory_backed_and_capped() {
    let size = |memory_mib| {
        runtime_tmp_volume(memory_mib)
            .empty_dir
            .and_then(|source| source.size_limit)
            .map(|quantity| quantity.0)
    };
    assert_eq!(runtime_tmp_volume(0).name, RUNTIME_TMP_VOLUME);
    assert_eq!(size(0), Some(format!("{RUNTIME_TMP_MAX_MIB}Mi")));
    assert_eq!(size(4), Some("4Mi".to_string()));
    assert_eq!(size(u32::MAX), Some(format!("{RUNTIME_TMP_MAX_MIB}Mi")));
    assert_eq!(
        runtime_tmp_volume(1)
            .empty_dir
            .and_then(|source| source.medium)
            .as_deref(),
        Some("Memory")
    );
}
