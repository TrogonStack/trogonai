//! The gateway's platform audit path (ADR#0086 Decisions 3 and 4).
//!
//! Every ingress decision becomes a `trogon-audit` record handed to an
//! [`AuditWriter`](trogon_audit::AuditWriter). The gateway never chooses how
//! durably a record is written: the record's facts do.
//!
//! | decision                                      | class       | a failed write                           |
//! |-----------------------------------------------|-------------|------------------------------------------|
//! | any deny                                      | must-audit  | the deny stands, the record is retried   |
//! | an allow of a method not declared read-only   | must-audit  | the request is refused, nothing forwards |
//! | an allow of a method declared read-only       | best-effort | queued, never refuses                    |
//!
//! The allow is recorded and acknowledged before the request is forwarded. A
//! refusal for a lost record is the JSON-RPC server error `-32804` carrying the
//! reason `audit_unavailable`, an operational fault and never a policy denial.
//!
//! The legacy `a2a-nats` envelope keeps publishing beside these records, best
//! effort, for deployments that still read `{prefix}.v1.audit.{agent}.{outcome}`.

mod audit_error;
mod audit_names;
mod config;
mod decision_event;
mod decision_record;
mod decision_site;
mod declared_read_only;
mod deny_reason;
mod jetstream;
mod writer_audit;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

pub use audit_error::AuditError;
pub use config::{PlatformAuditConfig, PlatformAuditConfigError};
pub use decision_event::{AllowedLayers, DecisionEvent, Verdict};
pub use decision_site::DecisionSite;
pub use declared_read_only::declared_read_only;
pub use deny_reason::DenyReason;
pub use jetstream::jetstream_audit_from_env;
pub use writer_audit::WriterAudit;

/// Where the gateway's decisions are recorded.
#[async_trait]
pub trait DecisionAudit: Send + Sync {
    /// Records one decision. `Err` for an allow means the request must not be
    /// forwarded. For a deny the answer is sent regardless, so the caller
    /// ignores `Err`.
    async fn record(&self, event: &DecisionEvent<'_>) -> Result<(), AuditError>;

    /// Background work the sink needs for as long as the gateway serves, such
    /// as retrying buffered records. Returns once `shutdown` is cancelled and
    /// the writes still in flight have settled.
    async fn run(&self, _shutdown: CancellationToken) {}
}
