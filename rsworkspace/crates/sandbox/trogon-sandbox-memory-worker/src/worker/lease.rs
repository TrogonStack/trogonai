use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use trogon_sandbox_contract::{
    ExecutionId, FileOpErrorKind, FileOpRequest, FileOpResponse, LeaseId, RunCommandRequest,
};
use trogon_sandbox_file_ops::{Manifest, Tracking};
use trogon_sandbox_protocol::unsupported_file_op_response;
use trogon_sandbox_protocol::{AuthorityResourceId, AuthorityTaskCompletion};

use super::command_task::CommandTask;
use super::exec_state::ExecState;
use super::memory_source::MemorySource;
use super::proc_entry::Proc;
use crate::constants::TRACKING_KEY;
use crate::mutex_ext::MutexExt;
use crate::{FrameSink, MemoryFs};

/// The authority identity a lease was granted under.
pub(crate) struct LeaseIncarnation {
    pub resource: AuthorityResourceId,
    pub completion: AuthorityTaskCompletion,
}

/// The state of one granted lease.
pub(crate) struct Lease {
    pub incarnation: LeaseIncarnation,
    pub closing: AtomicBool,
    pub admission: tokio::sync::Mutex<()>,
    pub tasks: Mutex<HashMap<String, Arc<CommandTask>>>,
    /// The region the lease asked for, reported back by the native handle.
    pub region: Option<String>,
    pub id: LeaseId,
    /// Checkpoints, the lease-start baseline and watches over `fs`.
    pub tracking: Arc<Tracking>,
    /// Set by `pause`: new commands and file writes are refused until `resume`.
    /// Files are kept; detached commands already running are not frozen.
    pub paused: AtomicBool,
    pub fs: Arc<Mutex<MemoryFs>>,
    /// Locked before `execs` when both are needed.
    pub procs: Mutex<HashMap<ExecutionId, Proc>>,
    pub execs: Mutex<HashMap<ExecutionId, ExecState>>,
    /// The request each detached execution started with, so a restart runs
    /// the same argv, cwd, env and process spec again.
    pub launches: Mutex<HashMap<ExecutionId, RunCommandRequest>>,
}

impl Lease {
    /// A lease whose `changes` baseline is the empty file map it starts with.
    pub(crate) fn new(id: LeaseId, region: Option<String>, incarnation: LeaseIncarnation) -> Self {
        let tracking = Arc::new(Tracking::new());
        tracking.set_start_baseline(TRACKING_KEY, "/", Manifest::new());
        Self {
            incarnation,
            closing: AtomicBool::new(false),
            admission: tokio::sync::Mutex::new(()),
            tasks: Mutex::new(HashMap::new()),
            region,
            id,
            tracking,
            paused: AtomicBool::new(false),
            fs: Arc::new(Mutex::new(MemoryFs::default())),
            procs: Mutex::new(HashMap::new()),
            execs: Mutex::new(HashMap::new()),
            launches: Mutex::new(HashMap::new()),
        }
    }

    /// Serves the change-tracking and watch verbs over the file map. The memory worker
    /// models nothing else of `file_op`, so every other verb is a typed Unsupported.
    pub(crate) async fn file_op(&self, body: &[u8]) -> FileOpResponse {
        let request = match serde_json::from_slice::<FileOpRequest>(body) {
            Ok(request) => request,
            Err(error) => {
                return FileOpResponse::Failed {
                    kind: FileOpErrorKind::InvalidRequest,
                    message: format!("invalid request: {error}"),
                };
            }
        };
        let source = MemorySource {
            fs: Arc::clone(&self.fs),
        };
        match self.tracking.serve(TRACKING_KEY, &request.op, &source).await {
            Some(reply) => reply,
            None => unsupported_file_op_response("mock", &request.op),
        }
    }

    /// Puts watch overflows on the lease's evidence timeline, as the real workers
    /// do. Best effort: a failed publish is logged and never fails the verb.
    pub(crate) async fn publish_watch_overflows(&self, sink: &dyn FrameSink) {
        for (subject, evidence) in self.tracking.take_overflow_evidence(TRACKING_KEY, &self.id) {
            let payload = serde_json::to_vec(&evidence).unwrap_or_default();
            sink.publish(subject, payload.into()).await;
        }
    }

    /// The exit code of `execution_id` if it finished, `None` while running.
    pub(crate) fn exit_code(&self, execution_id: &ExecutionId) -> Option<Option<i32>> {
        match self.execs.lock_clean().get(execution_id) {
            None => None,
            Some(ExecState::Running { .. }) => Some(None),
            Some(ExecState::Done(output)) => Some(Some(output.exit_code)),
        }
    }
}
