use std::sync::Arc;

use trogon_audit::{AuditWriter, JetStreamSink, WriterConfig};
use trogon_std::env::ReadEnv;

use super::DecisionAudit;
use super::config::{PlatformAuditConfig, PlatformAuditConfigError};
use super::writer_audit::WriterAudit;
use crate::runtime::env::gateway_audit_publish_enabled;

/// The platform audit writer over the stream the connection reaches, or `None`
/// when audit publishing is off.
///
/// Settings that cannot build a writer stop the process at start. A stream that
/// is configured and not answering does not: the gateway keeps denying what it
/// denies and refuses the requests it cannot record until the stream returns.
/// The drain task is not spawned here; [`DecisionAudit::run`] is.
pub fn jetstream_audit_from_env<E: ReadEnv>(
    env: &E,
    client: &async_nats::Client,
) -> Result<Option<Arc<dyn DecisionAudit>>, PlatformAuditConfigError> {
    if !gateway_audit_publish_enabled(env) {
        return Ok(None);
    }
    let config = PlatformAuditConfig::from_env(env)?;
    let sink = JetStreamSink::with_defaults(async_nats::jetstream::new(client.clone()));
    let writer = AuditWriter::new(sink, config.writer, WriterConfig::default());
    Ok(Some(Arc::new(WriterAudit::new(writer, config.tenant, config.project))))
}

#[cfg(test)]
mod tests;
