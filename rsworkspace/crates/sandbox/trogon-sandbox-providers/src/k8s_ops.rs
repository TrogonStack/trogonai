//! Kubernetes API operations, the data they exchange and the pure helpers over cluster objects.

mod api_server_clock;
mod capped_read;
mod claimed_warm_pod;
pub(crate) mod constants;
mod exit_code;
mod fenced_patch_outcome;
mod image_user_resolver;
mod interactive_pod_exec;
mod job_deadlines;
mod job_listing;
mod job_phase;
mod k8s_launch_options;
pub(crate) mod k8s_ops_trait;
mod launch_rules;
mod lease_job_sweep;
mod lease_owner_kind;
mod patch_bodies;
mod pod_exec_output;
mod pod_listing;
pub(crate) mod pod_readiness;
mod pod_security;
mod real_k8s_ops;
mod recovered_lease_job;
mod recovered_warm_lease;
mod secret_names;
mod tty_pod_exec;

pub use api_server_clock::{ApiServerClock, pod_age_seconds};
pub use claimed_warm_pod::ClaimedWarmPod;
pub use fenced_patch_outcome::FencedPatchOutcome;
pub use image_user_resolver::ImageUserResolver;
pub use interactive_pod_exec::InteractivePodExec;
pub use job_deadlines::{job_is_finished, job_outlived_deadline, warm_pod_outlived_deadline};
pub use job_listing::JobListing;
pub use k8s_launch_options::K8sLaunchOptions;
pub use launch_rules::{digest_label_value, policy_needs_egress_proxy};
pub use lease_job_sweep::LeaseJobSweep;
pub use lease_owner_kind::LeaseOwnerKind;
pub use pod_exec_output::PodExecOutput;
pub use pod_listing::PodListing;
pub use pod_readiness::pod_is_running_public;
pub use recovered_lease_job::RecoveredLeaseJob;
pub use recovered_warm_lease::RecoveredWarmLease;
pub use secret_names::{
    clone_secret_name, lease_job_secret_names, lease_request_secret_name, restore_secret_name, s3_volume_secret_name,
    warm_pod_restore_secret_name,
};
pub use tty_pod_exec::TtyPodExec;

pub(crate) use api_server_clock::parse_http_date;
pub(crate) use capped_read::{read_capped, read_tail};
pub(crate) use exit_code::exit_code_from_status;
pub(crate) use image_user_resolver::image_user_is_non_root_uid;
pub(crate) use job_deadlines::{job_deadline_seconds, job_outlived_deadline_by, warm_pod_outlived_deadline_by};
pub(crate) use job_phase::{JobPhase, job_phase_from_k8s};
pub(crate) use k8s_ops_trait::K8sOps;
pub(crate) use launch_rules::{is_strong_isolation_runtime_class, parse_run_as_user, runtime_class_name};
pub(crate) use pod_readiness::{JobPodReadiness, job_pod_readiness, pod_is_running};
pub(crate) use pod_security::{
    container_resources, hardened_pod_security_context, job_delete_params, keep_alive_command, runtime_tmp_volume,
};
pub(crate) use real_k8s_ops::RealK8sOps;

#[cfg(test)]
mod tests;
