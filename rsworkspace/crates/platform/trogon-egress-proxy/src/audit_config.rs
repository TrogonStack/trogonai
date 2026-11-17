use trogon_audit::WriterIdentity;
use trogon_nats::NatsConfig;

/// Where decisions are recorded and under whose name (ADR#0085 Decision 5).
///
/// Audit is not optional: the proxy only runs in enforce mode, so there is no
/// value of this type that means "off".
///
/// A proxy instance serves exactly one tenant and project, taken from
/// configuration. They come from here only until the certificate profile that
/// names them exists, and then the certificate replaces them.
#[derive(Debug, Clone)]
pub struct AuditConfig {
    /// The connection the audit stream is reached over. TLS is required unless
    /// `NATS_TLS_DISABLED` says otherwise, as for every NATS client here.
    pub nats: NatsConfig,
    /// The writer this process is to its chain, and the tenant and project its
    /// records are scoped to. The writer id is derived from the instance name,
    /// so the same instance is the same writer after a restart, and each process
    /// opens a fresh chain under it.
    pub identity: WriterIdentity,
}
