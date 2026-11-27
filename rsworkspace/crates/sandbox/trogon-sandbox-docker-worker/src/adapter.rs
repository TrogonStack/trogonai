mod active_docker_lease;
mod assigned;
mod authority;
mod chunk_sequencer;
mod command_control;
mod command_custody;
mod command_exec;
pub mod command_runtime;
mod command_task;
mod constants;
mod controlled;
mod docker_worker_provider;
mod exec_collect;
mod exec_created_callback;
mod exec_outcome;
mod execution_state;
mod file_op_class;
mod files;
mod fuse_binds;
mod launch_fingerprint;
mod lease;
mod lease_operations;
pub(crate) mod native_birth;
mod native_hardening;
mod output_sink;
mod output_tap;
mod process_meta;
mod provider_defaults;
mod provider_failure;
mod provider_impl;
mod rollback;
mod snapshot_restore;
mod spec_edit;
mod ssh;
mod stdin;
mod stored_launch;
mod streaming_context;
mod supervised_exec;
mod users_groups;
mod warm_pool_ops;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use anyhow::Context;
use async_nats::jetstream;
use async_trait::async_trait;
use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures::StreamExt;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    ArtifactType, CommandDeadline, CommandEnv, CommandOutput, CommandTimeoutStatus, CreateSandboxGroupResponse,
    CreateSandboxUserResponse, Digest, ExecutionId, GroupMembershipAction, KillCommandResponse, LeaseError, LeaseId,
    LeaseRequest, ListProcessesResponse, ModifyGroupMembershipResponse, NetworkPolicy, OciArtifactRef, OciImageRef,
    OutputRetention, OutputTruncation, PollCommandResponse, ProcessInfo, ProcessName, ProcessSpec, ProcessState,
    ProviderKind, ReadinessProbe, RemoteMount, RestartCommandResponse, SandboxFileWrite, SandboxGroupName, SandboxId,
    SandboxImage, SandboxUserName, SnapshotPushTarget, StdinChunk, StdinEof, StdinMode, StdinSeq, StopCommandResponse,
    StopSignal, StreamedOutput, UnixSignal, WaitReadyResponse, WorkspaceSource, WriteStdinResponse,
};
use trogon_sandbox_contract::{HostMountAccess, S3BucketName, VolumeMode};
use trogon_sandbox_contract::{ProviderLease, SnapshotReceipt};
use trogon_sandbox_cosign::{CosignVerifier, verify_if_configured};
use trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost};
use trogon_sandbox_protocol::RedactionSet;
use trogon_sandbox_protocol::{CappedOutput, FileNotFoundError, ReadFileError};
use trogon_sandbox_protocol::{Provider, ProviderError, lifecycle_subject};
use trogon_sandbox_protocol::{StdinAdmit, StdinSequencer, StdinWrite};
use trogon_sandbox_protocol::{home_dir_for_user, shared_dir_for_group};
use trogon_sandbox_volumes::{
    FuseMetrics, FuseOtherUsers, VolumeBackend, VolumeClient, VolumeClientConfig, VolumeEndpoint, VolumePrefix,
    VolumeRegion,
};
use trogon_sandbox_volumes::{FuseMountConfig, FuseMountHandle, FuseMountManager};

use crate::checkpoint::{
    checkpoint_name_from_annotations, checkpoint_storage_dir, gzip_checkpoint, tar_checkpoint_dir, untar_checkpoint,
};
use crate::digest_cache::DigestCache;
use crate::nats_lifecycle::NatsLifecycleEmitter;
use crate::registry_auth::login_for_pull_secret;
use crate::rollback_state::RollbackStateStore;
use crate::snapshot::{
    gunzip_tar, gzip_tar, layer_is_checkpoint, pull_snapshot_layer, push_checkpoint_artifact, push_rootfs_artifact,
};
use crate::warm_pool::{WarmContainer, WarmPool, WarmPoolConfig};

use trogon_sandbox_providers::{
    ContainerHardening, LocalDockerProvider, SandboxProvider, WORKSPACE_MOUNT_PATH, lease_cpus,
    translate_lease_request_to_sandbox_spec,
};
use trogon_sandbox_providers::{ContainerRunSpec, InspectStatus};

use command_task::CommandTask;
use constants::{
    CONTAINER_DISPOSAL_TIMEOUT, DEFAULT_VOLUME_REGION, EXEC_ID_POLL_INTERVAL, EXEC_ID_WAIT, EXECUTION_ID_ENV,
    NATIVE_OPTION_KEYS, OP_CANCELLED, OP_QUEUED, OP_STARTED, OUTPUT_SETTLE_GRACE, PROCESS_OUTPUT_TAIL,
    READY_POLL_INTERVAL, SIGNAL_EXEC_USER, STDIN_QUEUE_DEPTH, STDIN_STARTED_WRITE_DEADLINE, STDIN_WRITE_DEADLINE,
    STOP_KILL_SETTLE, STOP_POLL_INTERVAL, STREAM_ONLY_STDERR_TAIL, TCP_LISTEN_SCRIPT,
};
use file_op_class::FileOpClass;
use provider_failure::into_provider_error;

use active_docker_lease::ActiveDockerLease;
use chunk_sequencer::ChunkSequencer;
pub use docker_worker_provider::DockerWorkerProvider;
use exec_created_callback::ExecCreatedCallback;
use exec_outcome::{SignalOutcome, TimeoutCleanup};
use execution_state::ExecutionState;
#[cfg(test)]
use files::cat_output_into_file_bytes;
pub use fuse_binds::build_fuse_bind_strings;
#[cfg(test)]
use lease_operations::{gauges_from_stats, join_relative_cwd};
pub(crate) use native_hardening::hardening_with_native;
use output_sink::OutputSink;
use output_tap::OutputTap;
use process_meta::ProcessMeta;
pub use provider_defaults::{build_oci_client, default_warm_pool_config};
use snapshot_restore::restored_image_tag;
use stdin::{ExecStdin, StdinApplyError, StdinHandle, StdinOp, run_stdin_writer};
use stored_launch::StoredLaunch;
use streaming_context::StreamingContext;
use trogon_sandbox_protocol::SIGNAL_BY_MARKER_SCRIPT;
pub use trogon_sandbox_providers::translate_lease_request_to_sandbox_spec as translate_spec;

#[cfg(test)]
mod deadline_tests;

#[cfg(test)]
mod tests;
