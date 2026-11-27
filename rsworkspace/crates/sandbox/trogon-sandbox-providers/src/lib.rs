//! Docker, Kubernetes microVM and subprocess sandbox providers shared by the sandbox workers.

mod artifact_ref;
mod artifact_sink;
mod artifact_sink_error;
mod clone_cache;
mod clone_credentials;
mod commit_sha;
mod commit_sha_error;
mod docker;
mod docker_engine;
mod docker_engine_bollard;
mod docker_engine_checkpoint_create_spec;
mod docker_engine_checkpoint_restore_spec;
mod docker_engine_container_hardening;
mod docker_engine_container_run_spec;
mod docker_engine_container_start_result;
mod docker_engine_inspect_status;
mod docker_error;
mod egress_lockdown_rules;
mod egress_policy;
mod egress_proxy_token;
mod egress_redirect;
mod egress_rules_failure;
mod egress_violation_events;
mod git_command;
mod git_provider;
mod git_step;
mod host_mount;
mod image_label;
mod image_ref;
mod k8s_error;
mod k8s_microvm_config;
mod k8s_microvm_handle;
mod k8s_microvm_provider;
mod k8s_microvm_provider_exec;
mod k8s_microvm_provider_launch;
mod k8s_microvm_provider_secrets;
mod k8s_microvm_provider_sweep;
mod k8s_microvm_provider_warm_pool;
#[cfg(test)]
mod k8s_microvm_tests;
mod k8s_network_policy;
mod k8s_network_policy_egress_sidecar_resources;
mod k8s_network_policy_error;
mod k8s_ops;
mod k8s_ops_error;
mod k8s_ops_run_as_user_error;
mod k8s_ownership_cluster;
mod k8s_ownership_error;
mod k8s_ownership_lease;
mod k8s_ownership_provider;
mod k8s_ownership_receipt;
mod k8s_ownership_resource_kind;
mod k8s_provider_error;
mod k8s_secrets;
mod k8s_secrets_error;
mod lease_spec;
mod lease_spec_error;
mod local_docker_egress;
mod local_docker_handle;
mod local_docker_launch;
mod local_docker_policy;
mod local_docker_provider;
mod local_docker_sidecar_launch;
mod local_docker_sidecar_ready;
mod local_docker_snapshot;
mod local_docker_workspace;
mod noop_artifact_sink;
mod process_spawner;
mod proxy_uid_workload_error;
mod real_process_spawner;
mod repo_ref;
mod resource_naming;
mod resource_naming_error;
mod resource_naming_job_name;
mod sandbox_error;
mod sandbox_handle;
mod sandbox_provider;
mod sandbox_spec;
mod sandbox_status;
mod sink_host;
mod spawned_process;
mod subprocess_clone;
mod subprocess_error;
mod subprocess_sandbox_handle;
mod subprocess_sandbox_provider;
mod tool_selector;
mod tool_spec;
mod violation_sink_endpoint;
mod violation_sink_endpoint_error;
mod volume_cache_env;
mod volume_cache_error;

pub use artifact_ref::ArtifactRef;
pub use artifact_sink::ArtifactSink;
pub use artifact_sink_error::ArtifactSinkError;
pub use clone_cache::{clone_credentials, try_clone_with_volume_cache};
pub use clone_credentials::CloneCredentials;
pub use commit_sha::CommitSha;
pub use commit_sha_error::CommitShaError;
pub use docker::constants::{
    DEFAULT_DOCKER_SOCKET, DOCKER_LABEL_LEASE_ID, DOCKER_LABEL_OWNER, DOCKER_LABEL_ROLE, WORKSPACE_MOUNT_PATH,
};
pub use docker_engine::DockerEngine;
pub use docker_engine_bollard::BollardEngine;
pub use docker_engine_checkpoint_create_spec::CheckpointCreateSpec;
pub use docker_engine_checkpoint_restore_spec::CheckpointRestoreSpec;
pub use docker_engine_container_hardening::ContainerHardening;
pub use docker_engine_container_run_spec::ContainerRunSpec;
pub use docker_engine_container_start_result::ContainerStartResult;
pub use docker_engine_inspect_status::InspectStatus;
pub use docker_error::DockerError;
pub use egress_lockdown_rules::EgressLockdownRules;
pub use egress_policy::EgressPolicy;
pub(crate) use egress_proxy_token::EgressProxyToken;
pub use egress_redirect::{
    EGRESS_PROXY_EXEMPT_UID_ENV, EGRESS_PROXY_LEASE_ID_ENV, EGRESS_PROXY_VIOLATION_NATS_URL_ENV,
    UID_CHANGE_CAPABILITIES, egress_lockdown_command, egress_lockdown_rules, egress_redirect_command,
    exempt_uid_env_pair, reject_proxy_uid_workload, rewrite_loopback_host,
};
pub use egress_rules_failure::EgressRulesFailure;
pub use egress_violation_events::EgressViolationEvents;
pub use git_provider::GitProvider;
pub use git_step::GitStep;
pub use host_mount::HostMount;
pub use image_ref::ImageRef;
pub use k8s_error::K8sError;
pub use k8s_microvm_config::K8sMicroVmConfig;
pub use k8s_microvm_provider::K8sMicroVmProvider;
pub use k8s_network_policy::{
    EGRESS_PROXY_CA_MOUNT_PATH, EGRESS_PROXY_CA_VOLUME_NAME, EGRESS_PROXY_IMAGE, build_egress_sidecar,
    build_network_policy_manifest, egress_proxy_configmap_name, network_policy_name,
};
pub use k8s_network_policy_egress_sidecar_resources::EgressSidecarResources;
pub use k8s_network_policy_error::K8sNetworkPolicyError;
pub use k8s_ops::constants::{
    ANNOTATION_DIGEST, ANNOTATION_SOURCE_REGISTRY, ANNOTATION_SOURCE_REPOSITORY, DEFAULT_AGENT_UID,
    FOREIGN_LEASE_GRACE_SECONDS, LABEL_DIGEST, LABEL_LEASE_DEADLINE, LABEL_LEASE_ID, LABEL_TENANT_ID, LABEL_WARM_POOL,
    LABEL_WORKER_IDENTITY, ORPHAN_RECLAIM_GRACE_SECONDS, WARM_POOL_CLAIMED, WARM_POOL_IDLE, WORKSPACE_FS_GROUP,
};
#[cfg(test)]
pub(crate) use k8s_ops::constants::{
    EVIDENCE_PATH, IMAGE_USER_CACHE_TTL, RESTORE_VOLUME_NAME, SANDBOX_TERMINATION_GRACE_SECONDS, SECCOMP_PROFILE_TYPE,
};
pub(crate) use k8s_ops::constants::{JOB_POD_POLL_INTERVAL, LEASE_REQUEST_KEY};
pub(crate) use k8s_ops::k8s_ops_trait::K8sOps;
pub(crate) use k8s_ops::pod_readiness::{JobPodReadiness, job_pod_readiness, pod_is_running};
pub use k8s_ops::{
    ApiServerClock, ClaimedWarmPod, FencedPatchOutcome, ImageUserResolver, InteractivePodExec, JobListing,
    K8sLaunchOptions, LeaseJobSweep, LeaseOwnerKind, PodExecOutput, PodListing, RecoveredLeaseJob, RecoveredWarmLease,
    TtyPodExec, clone_secret_name, digest_label_value, job_is_finished, job_outlived_deadline, lease_job_secret_names,
    lease_request_secret_name, pod_age_seconds, pod_is_running_public, policy_needs_egress_proxy, restore_secret_name,
    s3_volume_secret_name, warm_pod_outlived_deadline, warm_pod_restore_secret_name,
};
pub use k8s_ops_error::K8sOpsError;
pub use k8s_ops_run_as_user_error::RunAsUserError;
pub use k8s_ownership_error::K8sOwnershipError;
pub use k8s_ownership_lease::ConfirmedK8sLease;
pub use k8s_ownership_receipt::K8sResourceReceipt;
pub use k8s_ownership_resource_kind::K8sResourceKind;
pub use k8s_provider_error::K8sProviderError;
pub(crate) use k8s_secrets::{
    build_clone_secret, build_git_init_container, build_workspace_import_init_container,
    build_workspace_tar_secret_with_naming,
};
pub use k8s_secrets::{build_workspace_tar_secret, resolve_secret_bytes};
pub use k8s_secrets_error::K8sSecretError;
pub use lease_spec::{DEFAULT_LEASE_CPUS, lease_cpus, lease_timeout, translate_lease_request_to_sandbox_spec};
pub use lease_spec_error::LeaseSpecError;
pub use local_docker_handle::LocalDockerHandle;
pub use local_docker_provider::LocalDockerProvider;
pub use noop_artifact_sink::NoopArtifactSink;
pub use process_spawner::ProcessSpawner;
pub use proxy_uid_workload_error::ProxyUidWorkloadError;
pub use real_process_spawner::RealProcessSpawner;
pub use repo_ref::RepoRef;
pub use resource_naming::K8sResourceNaming;
pub use resource_naming_error::K8sResourceNamingError;
pub use resource_naming_job_name::job_name;
pub use sandbox_error::SandboxError;
pub use sandbox_handle::SandboxHandle;
pub use sandbox_provider::SandboxProvider;
pub use sandbox_spec::SandboxSpec;
pub use sandbox_status::SandboxStatus;
pub use sink_host::SinkHost;
pub use spawned_process::SpawnedProcess;
pub use subprocess_error::SubprocessError;
pub use subprocess_sandbox_handle::SubprocessSandboxHandle;
pub use subprocess_sandbox_provider::SubprocessSandboxProvider;
pub use tool_selector::ToolSelector;
pub use tool_spec::ToolSpec;
pub use violation_sink_endpoint::ViolationSinkEndpoint;
pub use violation_sink_endpoint_error::ViolationSinkEndpointError;
pub use volume_cache_env::VolumeCacheEnv;
pub use volume_cache_error::VolumeCacheError;
