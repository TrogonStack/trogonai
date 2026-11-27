use trogon_sandbox_contract::{LeaseId, WorkerId};

/// Which kind of signed broker-to-worker request a subject carries.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SignedSubject {
    /// A per-lease verb.
    LeaseVerb {
        /// The lease id in the subject.
        lease: LeaseId,
    },
    /// `sandbox.provider.<provider>.worker.<worker>.command_readiness.<lease>`.
    WorkerCommandReadiness {
        /// The provider token in the subject.
        provider: String,
        /// The worker the probe is addressed to.
        worker: WorkerId,
        /// The lease id in the subject.
        lease: LeaseId,
    },
    /// `sandbox.provider.<provider>.worker.<worker>.cleanup`.
    Cleanup,
    /// `sandbox.provider.<provider>.purge_named`: tenant-wide, no lease.
    PurgeNamed,
    /// `sandbox.ssh.attach.<lease>` or `sandbox.ssh.detach.<lease>`, sent by the ssh gateway on behalf of an
    /// authenticated tenant.
    SshSession {
        /// The lease id in the subject.
        lease: LeaseId,
    },
    /// `sandbox.worker.<worker>.snapshot.export`: names its lease in the body.
    SnapshotExport,
    /// `sandbox.provider.<provider>.native_snapshot_delete`.
    NativeSnapshotDelete {
        /// The provider token in the subject.
        provider: String,
    },
}
